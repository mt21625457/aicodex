use std::collections::BTreeSet;
use std::fmt;

use serde_json::Value;

pub const CLAUDE_WEB_SEARCH_20250305_TOOL_TYPE: &str = "web_search_20250305";
pub const CLAUDE_WEB_SEARCH_20260209_TOOL_TYPE: &str = "web_search_20260209";
pub const CLAUDE_WEB_FETCH_20250910_TOOL_TYPE: &str = "web_fetch_20250910";
pub const CLAUDE_WEB_FETCH_20260209_TOOL_TYPE: &str = "web_fetch_20260209";
pub const CLAUDE_CODE_EXECUTION_20250825_TOOL_TYPE: &str = "code_execution_20250825";
pub const CLAUDE_CODE_EXECUTION_20260120_TOOL_TYPE: &str = "code_execution_20260120";
pub const CLAUDE_ADVISOR_20260301_TOOL_TYPE: &str = "advisor_20260301";
pub const CLAUDE_TOOL_SEARCH_REGEX_20251119_TOOL_TYPE: &str = "tool_search_tool_regex_20251119";
pub const CLAUDE_TOOL_SEARCH_BM25_20251119_TOOL_TYPE: &str = "tool_search_tool_bm25_20251119";
pub const CLAUDE_MCP_TOOLSET_TOOL_TYPE: &str = "mcp_toolset";
pub const CLAUDE_MEMORY_20250818_TOOL_TYPE: &str = "memory_20250818";
pub const CLAUDE_BASH_20250124_TOOL_TYPE: &str = "bash_20250124";
pub const CLAUDE_TEXT_EDITOR_20250728_TOOL_TYPE: &str = "text_editor_20250728";
pub const CLAUDE_TEXT_EDITOR_20250124_TOOL_TYPE: &str = "text_editor_20250124";
pub const CLAUDE_COMPUTER_20251124_TOOL_TYPE: &str = "computer_20251124";
pub const CLAUDE_COMPUTER_20250124_TOOL_TYPE: &str = "computer_20250124";
pub const CLAUDE_TEXT_EDITOR_TOOL_NAME: &str = "str_replace_based_edit_tool";
pub const CLAUDE_BASH_TOOL_NAME: &str = "bash";
pub const CLAUDE_MEMORY_TOOL_NAME: &str = "memory";
pub const CLAUDE_COMPUTER_TOOL_NAME: &str = "computer";
pub const CLAUDE_WEB_SEARCH_TOOL_NAME: &str = "web_search";
pub const CLAUDE_WEB_FETCH_TOOL_NAME: &str = "web_fetch";
pub const CLAUDE_CODE_EXECUTION_TOOL_NAME: &str = "code_execution";
pub const CLAUDE_ADVISOR_TOOL_NAME: &str = "advisor";
pub const CLAUDE_TOOL_SEARCH_TOOL_NAME: &str = "tool_search";
pub const CLAUDE_TOOL_SEARCH_REGEX_TOOL_NAME: &str = "tool_search_tool_regex";
pub const CLAUDE_TOOL_SEARCH_BM25_TOOL_NAME: &str = "tool_search_tool_bm25";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ClaudeProviderPlatform {
    #[default]
    AnthropicApi,
    Bedrock,
    Vertex,
    MicrosoftFoundry,
    DeepSeekCompatible,
    Unknown,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ClaudeLocalExecutorCapability {
    #[default]
    Unavailable,
    Available,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ClaudeServerCapability {
    #[default]
    Disabled,
    Enabled,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClaudeNativeToolSelection {
    pub provider_platform: ClaudeProviderPlatform,
    pub model: Option<String>,
    pub allowed_tools: BTreeSet<ClaudeNativeToolKind>,
    pub remote_mcp_servers: Vec<ClaudeMcpServer>,
    pub advisor_model: Option<String>,
    pub advisor_max_uses: Option<u32>,
    pub require_zero_data_retention: bool,
    pub text_editor_executor: ClaudeLocalExecutorCapability,
    pub bash_executor: ClaudeLocalExecutorCapability,
    pub memory_executor: ClaudeLocalExecutorCapability,
    pub computer_executor: ClaudeLocalExecutorCapability,
    pub server_dynamic_filtering: ClaudeServerCapability,
    pub remote_mcp_connector: ClaudeServerCapability,
    pub tool_search: ClaudeServerCapability,
}

impl ClaudeNativeToolSelection {
    pub fn allows(&self, tool: ClaudeNativeToolKind) -> bool {
        self.allowed_tools.contains(&tool)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ClaudeNativeToolKind {
    WebSearch20250305,
    WebSearch20260209,
    WebFetch20250910,
    WebFetch20260209,
    CodeExecution20250825,
    CodeExecution20260120,
    Advisor20260301,
    ToolSearchRegex20251119,
    ToolSearchBm25V20251119,
    McpToolset,
    Memory20250818,
    Bash20250124,
    TextEditor20250728,
    TextEditor20250124,
    Computer20251124,
    Computer20250124,
}

impl ClaudeNativeToolKind {
    pub fn tool_type(self) -> &'static str {
        match self {
            Self::WebSearch20250305 => CLAUDE_WEB_SEARCH_20250305_TOOL_TYPE,
            Self::WebSearch20260209 => CLAUDE_WEB_SEARCH_20260209_TOOL_TYPE,
            Self::WebFetch20250910 => CLAUDE_WEB_FETCH_20250910_TOOL_TYPE,
            Self::WebFetch20260209 => CLAUDE_WEB_FETCH_20260209_TOOL_TYPE,
            Self::CodeExecution20250825 => CLAUDE_CODE_EXECUTION_20250825_TOOL_TYPE,
            Self::CodeExecution20260120 => CLAUDE_CODE_EXECUTION_20260120_TOOL_TYPE,
            Self::Advisor20260301 => CLAUDE_ADVISOR_20260301_TOOL_TYPE,
            Self::ToolSearchRegex20251119 => CLAUDE_TOOL_SEARCH_REGEX_20251119_TOOL_TYPE,
            Self::ToolSearchBm25V20251119 => CLAUDE_TOOL_SEARCH_BM25_20251119_TOOL_TYPE,
            Self::McpToolset => CLAUDE_MCP_TOOLSET_TOOL_TYPE,
            Self::Memory20250818 => CLAUDE_MEMORY_20250818_TOOL_TYPE,
            Self::Bash20250124 => CLAUDE_BASH_20250124_TOOL_TYPE,
            Self::TextEditor20250728 => CLAUDE_TEXT_EDITOR_20250728_TOOL_TYPE,
            Self::TextEditor20250124 => CLAUDE_TEXT_EDITOR_20250124_TOOL_TYPE,
            Self::Computer20251124 => CLAUDE_COMPUTER_20251124_TOOL_TYPE,
            Self::Computer20250124 => CLAUDE_COMPUTER_20250124_TOOL_TYPE,
        }
    }

    pub fn execution(self) -> ClaudeNativeToolExecution {
        match self {
            Self::WebSearch20250305
            | Self::WebSearch20260209
            | Self::WebFetch20250910
            | Self::WebFetch20260209
            | Self::CodeExecution20250825
            | Self::CodeExecution20260120
            | Self::Advisor20260301
            | Self::ToolSearchRegex20251119
            | Self::ToolSearchBm25V20251119
            | Self::McpToolset => ClaudeNativeToolExecution::Server,
            Self::Memory20250818
            | Self::Bash20250124
            | Self::TextEditor20250728
            | Self::TextEditor20250124
            | Self::Computer20251124
            | Self::Computer20250124 => ClaudeNativeToolExecution::Client,
        }
    }

    pub fn beta_feature(self) -> Option<ClaudeBetaFeature> {
        match self {
            Self::Advisor20260301 => Some(ClaudeBetaFeature::AdvisorTool20260301),
            Self::McpToolset => Some(ClaudeBetaFeature::McpClient20251120),
            Self::Computer20251124 => Some(ClaudeBetaFeature::ComputerUse20251124),
            Self::Computer20250124 => Some(ClaudeBetaFeature::ComputerUse20250124),
            Self::WebSearch20250305
            | Self::WebSearch20260209
            | Self::WebFetch20250910
            | Self::WebFetch20260209
            | Self::CodeExecution20250825
            | Self::CodeExecution20260120
            | Self::ToolSearchRegex20251119
            | Self::ToolSearchBm25V20251119
            | Self::Memory20250818
            | Self::Bash20250124
            | Self::TextEditor20250728
            | Self::TextEditor20250124 => None,
        }
    }

    pub fn requires_claude_api(self) -> bool {
        matches!(
            self,
            Self::Advisor20260301
                | Self::McpToolset
                | Self::ToolSearchRegex20251119
                | Self::ToolSearchBm25V20251119
                | Self::Computer20251124
                | Self::Computer20250124
        )
    }

    pub fn eligible_for_zero_data_retention(self) -> bool {
        matches!(
            self,
            Self::Memory20250818
                | Self::Bash20250124
                | Self::TextEditor20250728
                | Self::TextEditor20250124
                | Self::Computer20251124
                | Self::Computer20250124
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaudeNativeToolExecution {
    Client,
    Server,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ClaudeBetaFeature {
    AdvisorTool20260301,
    ComputerUse20251124,
    ComputerUse20250124,
    McpClient20251120,
}

impl ClaudeBetaFeature {
    pub fn header_value(self) -> &'static str {
        match self {
            Self::AdvisorTool20260301 => "advisor-tool-2026-03-01",
            Self::ComputerUse20251124 => "computer-use-2025-11-24",
            Self::ComputerUse20250124 => "computer-use-2025-01-24",
            Self::McpClient20251120 => "mcp-client-2025-11-20",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClaudeNativeToolPolicy {
    pub enabled_tools: BTreeSet<ClaudeNativeToolKind>,
    pub fallback_tools: BTreeSet<ClaudeNativeToolKind>,
    pub disabled_tools: BTreeSet<ClaudeNativeToolKind>,
    pub decisions: Vec<ClaudeNativeToolDecision>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeNativeToolDecision {
    pub tool: ClaudeNativeToolKind,
    pub outcome: ClaudeNativeToolDecisionOutcome,
    pub reason: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaudeNativeToolDecisionOutcome {
    Enabled,
    Fallback,
    Disabled,
}

pub fn evaluate_native_tool(
    tool: ClaudeNativeToolKind,
    selection: &ClaudeNativeToolSelection,
) -> ClaudeNativeToolDecision {
    if !selection.allows(tool) {
        return disabled(tool, "tool is not allowed by Claude native tool policy");
    }
    if tool.requires_claude_api()
        && selection.provider_platform != ClaudeProviderPlatform::AnthropicApi
    {
        return disabled(
            tool,
            "tool requires Anthropic API native support for this planning phase",
        );
    }
    if selection.require_zero_data_retention && !tool.eligible_for_zero_data_retention() {
        return disabled(tool, "tool is not eligible for zero data retention");
    }
    if !platform_supports_tool(tool, selection.provider_platform) {
        return disabled(
            tool,
            "provider platform does not support this Claude native tool",
        );
    }
    if !model_supports_tool(tool, selection.model.as_deref()) {
        return disabled(
            tool,
            "model does not support this Claude native tool version",
        );
    }

    match tool {
        ClaudeNativeToolKind::WebSearch20250305 => {
            enabled(tool, "Claude native server web search selected")
        }
        ClaudeNativeToolKind::WebSearch20260209 | ClaudeNativeToolKind::WebFetch20260209
            if selection.server_dynamic_filtering == ClaudeServerCapability::Enabled =>
        {
            enabled(tool, "Claude server dynamic filtering capability selected")
        }
        ClaudeNativeToolKind::WebFetch20250910 => {
            enabled(tool, "Claude native server web fetch selected")
        }
        ClaudeNativeToolKind::CodeExecution20250825
        | ClaudeNativeToolKind::CodeExecution20260120 => {
            enabled(tool, "Claude native server code execution selected")
        }
        ClaudeNativeToolKind::Advisor20260301
            if selection
                .advisor_model
                .as_deref()
                .is_some_and(is_opus_47_model) =>
        {
            enabled(tool, "Claude advisor tool selected")
        }
        ClaudeNativeToolKind::ToolSearchRegex20251119
        | ClaudeNativeToolKind::ToolSearchBm25V20251119
            if selection.tool_search == ClaudeServerCapability::Enabled =>
        {
            enabled(tool, "Claude native tool search selected")
        }
        ClaudeNativeToolKind::McpToolset
            if selection.remote_mcp_connector == ClaudeServerCapability::Enabled =>
        {
            enabled(tool, "Claude native remote MCP connector selected")
        }
        ClaudeNativeToolKind::TextEditor20250728 | ClaudeNativeToolKind::TextEditor20250124
            if selection.text_editor_executor == ClaudeLocalExecutorCapability::Available =>
        {
            enabled(tool, "Claude native text editor executor available")
        }
        ClaudeNativeToolKind::Bash20250124
            if selection.bash_executor == ClaudeLocalExecutorCapability::Available =>
        {
            enabled(tool, "Claude native bash executor available")
        }
        ClaudeNativeToolKind::Memory20250818
            if selection.memory_executor == ClaudeLocalExecutorCapability::Available =>
        {
            enabled(tool, "Claude native memory executor available")
        }
        ClaudeNativeToolKind::Computer20251124 | ClaudeNativeToolKind::Computer20250124
            if selection.computer_executor == ClaudeLocalExecutorCapability::Available =>
        {
            enabled(tool, "Claude native computer executor available")
        }
        ClaudeNativeToolKind::WebSearch20260209 | ClaudeNativeToolKind::WebFetch20260209 => {
            disabled(tool, "server dynamic filtering capability is disabled")
        }
        ClaudeNativeToolKind::ToolSearchRegex20251119
        | ClaudeNativeToolKind::ToolSearchBm25V20251119 => {
            disabled(tool, "Claude native tool search capability is disabled")
        }
        ClaudeNativeToolKind::McpToolset => {
            disabled(tool, "Claude native remote MCP connector is disabled")
        }
        ClaudeNativeToolKind::Advisor20260301 => {
            disabled(tool, "Claude advisor requires a supported advisor model")
        }
        ClaudeNativeToolKind::TextEditor20250728 | ClaudeNativeToolKind::TextEditor20250124 => {
            disabled(tool, "Claude native text editor executor is unavailable")
        }
        ClaudeNativeToolKind::Bash20250124 => {
            disabled(tool, "Claude native bash executor is unavailable")
        }
        ClaudeNativeToolKind::Memory20250818 => {
            disabled(tool, "Claude native memory executor is unavailable")
        }
        ClaudeNativeToolKind::Computer20251124 | ClaudeNativeToolKind::Computer20250124 => {
            disabled(tool, "Claude native computer executor is unavailable")
        }
    }
}

fn platform_supports_tool(tool: ClaudeNativeToolKind, platform: ClaudeProviderPlatform) -> bool {
    match platform {
        ClaudeProviderPlatform::AnthropicApi => true,
        ClaudeProviderPlatform::MicrosoftFoundry => matches!(
            tool,
            ClaudeNativeToolKind::WebSearch20250305
                | ClaudeNativeToolKind::WebSearch20260209
                | ClaudeNativeToolKind::WebFetch20250910
                | ClaudeNativeToolKind::WebFetch20260209
                | ClaudeNativeToolKind::CodeExecution20250825
                | ClaudeNativeToolKind::CodeExecution20260120
        ),
        ClaudeProviderPlatform::Vertex => matches!(tool, ClaudeNativeToolKind::WebSearch20250305),
        ClaudeProviderPlatform::Bedrock
        | ClaudeProviderPlatform::DeepSeekCompatible
        | ClaudeProviderPlatform::Unknown => false,
    }
}

fn model_supports_tool(tool: ClaudeNativeToolKind, model: Option<&str>) -> bool {
    let Some(model) = model else {
        return true;
    };
    match tool {
        ClaudeNativeToolKind::WebSearch20260209 | ClaudeNativeToolKind::WebFetch20260209 => {
            supports_dynamic_filtering_model(model)
        }
        ClaudeNativeToolKind::CodeExecution20260120 => {
            supports_code_execution_20260120_model(model)
        }
        ClaudeNativeToolKind::CodeExecution20250825 => {
            supports_code_execution_20250825_model(model)
        }
        ClaudeNativeToolKind::Advisor20260301 => supports_advisor_executor_model(model),
        ClaudeNativeToolKind::ToolSearchRegex20251119
        | ClaudeNativeToolKind::ToolSearchBm25V20251119 => is_claude_4_or_newer_model(model),
        ClaudeNativeToolKind::TextEditor20250728 => is_claude_4_or_newer_model(model),
        ClaudeNativeToolKind::TextEditor20250124 => !is_claude_4_or_newer_model(model),
        ClaudeNativeToolKind::WebSearch20250305
        | ClaudeNativeToolKind::WebFetch20250910
        | ClaudeNativeToolKind::McpToolset
        | ClaudeNativeToolKind::Memory20250818
        | ClaudeNativeToolKind::Bash20250124
        | ClaudeNativeToolKind::Computer20251124
        | ClaudeNativeToolKind::Computer20250124 => true,
    }
}

fn supports_dynamic_filtering_model(model: &str) -> bool {
    let model = model.to_ascii_lowercase();
    model.contains("mythos")
        || model.contains("opus-4-7")
        || model.contains("opus-4-6")
        || model.contains("sonnet-4-6")
}

fn supports_code_execution_20260120_model(model: &str) -> bool {
    let model = model.to_ascii_lowercase();
    model.contains("opus-4-7")
        || model.contains("opus-4-6")
        || model.contains("sonnet-4-6")
        || model.contains("opus-4-5")
        || model.contains("sonnet-4-5")
}

fn supports_code_execution_20250825_model(model: &str) -> bool {
    is_claude_4_or_newer_model(model)
}

fn supports_advisor_executor_model(model: &str) -> bool {
    let model = model.to_ascii_lowercase();
    model.contains("haiku-4-5")
        || model.contains("sonnet-4-6")
        || model.contains("opus-4-6")
        || model.contains("opus-4-7")
}

fn is_opus_47_model(model: &str) -> bool {
    model.to_ascii_lowercase().contains("opus-4-7")
}

fn is_claude_4_or_newer_model(model: &str) -> bool {
    let model = model.to_ascii_lowercase();
    model.contains("mythos")
        || model.contains("opus-4")
        || model.contains("sonnet-4")
        || model.contains("haiku-4")
}

fn enabled(tool: ClaudeNativeToolKind, reason: &'static str) -> ClaudeNativeToolDecision {
    ClaudeNativeToolDecision {
        tool,
        outcome: ClaudeNativeToolDecisionOutcome::Enabled,
        reason,
    }
}

fn disabled(tool: ClaudeNativeToolKind, reason: &'static str) -> ClaudeNativeToolDecision {
    ClaudeNativeToolDecision {
        tool,
        outcome: ClaudeNativeToolDecisionOutcome::Disabled,
        reason,
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClaudeHistoryRequirements {
    pub preserve_server_tool_results: bool,
    pub preserve_mcp_tool_results: bool,
    pub preserve_structured_citations: bool,
}

#[derive(Clone, PartialEq, Eq)]
pub struct ClaudeMcpServer {
    pub name: String,
    pub url: String,
    pub authorization_token: Option<String>,
    pub toolset_config: ClaudeMcpToolsetConfig,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClaudeMcpToolsetConfig {
    pub default_config: Option<Value>,
    pub configs: Option<Value>,
    pub allowed_tools: Option<Vec<String>>,
    pub denied_tools: Option<Vec<String>>,
    pub defer_loading: Option<bool>,
    pub cache_control: Option<Value>,
}

impl fmt::Debug for ClaudeMcpServer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClaudeMcpServer")
            .field("name", &self.name)
            .field("url", &self.url)
            .field(
                "authorization_token",
                &self.authorization_token.as_ref().map(|_| "[redacted]"),
            )
            .finish()
    }
}
