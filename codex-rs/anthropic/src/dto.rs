use codex_model_provider_info::ModelProviderInfo;
use codex_protocol::config_types::ReasoningSummary;
use codex_protocol::config_types::ServiceTier;
use codex_protocol::models::BaseInstructions;
use codex_protocol::models::ResponseItem;
use codex_protocol::openai_models::ReasoningEffort;
use codex_tools::ToolSpec;
use serde_json::Value;

#[derive(Debug, Clone)]
pub struct AnthropicTurnRequest {
    pub provider: ModelProviderInfo,
    pub model: String,
    pub input: Vec<ResponseItem>,
    pub tools: Vec<ToolSpec>,
    pub parallel_tool_calls: bool,
    pub base_instructions: BaseInstructions,
    pub effort: Option<ReasoningEffort>,
    pub summary: ReasoningSummary,
    pub service_tier: Option<ServiceTier>,
    pub turn_metadata_header: Option<String>,
    pub output_schema: Option<Value>,
}
