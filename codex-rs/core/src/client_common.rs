pub use codex_api::ResponseEvent;
use codex_protocol::error::Result;
use codex_protocol::models::BaseInstructions;
use codex_protocol::models::ContentItem;
use codex_protocol::models::DEFAULT_IMAGE_DETAIL;
use codex_protocol::models::FunctionCallOutputBody;
use codex_protocol::models::FunctionCallOutputContentItem;
use codex_protocol::models::ImageDetail;
use codex_protocol::models::ResponseItem;
use codex_protocol::openai_models::ModelInfo;
use codex_tools::ToolSpec;
use futures::Stream;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashSet;
use std::pin::Pin;
use std::sync::Arc;
use std::task::Context;
use std::task::Poll;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// API request payload for a single model turn
#[derive(Debug, Clone)]
pub struct Prompt {
    /// Conversation context input items.
    pub input: Vec<ResponseItem>,

    /// Tools available to the model, including additional tools sourced from
    /// external MCP servers.
    pub(crate) tools: Arc<[ToolSpec]>,

    /// Whether parallel tool calls are permitted for this prompt.
    pub(crate) parallel_tool_calls: bool,

    pub base_instructions: BaseInstructions,

    /// Optional the output schema for the model's response.
    pub output_schema: Option<Value>,

    /// Whether the Responses API should strictly validate `output_schema`.
    pub output_schema_strict: bool,

    pub(crate) cyber_access_program: Option<codex_protocol::turn_input::CyberAccessProgram>,
}

impl Default for Prompt {
    fn default() -> Self {
        Self {
            input: Vec::new(),
            tools: Arc::from([]),
            parallel_tool_calls: false,
            base_instructions: BaseInstructions::default(),
            output_schema: None,
            output_schema_strict: true,
            cyber_access_program: None,
        }
    }
}

impl Prompt {
    pub(crate) fn get_formatted_input_for_request(
        &self,
        model_info: &ModelInfo,
    ) -> Vec<ResponseItem> {
        let mut input = self.input.clone();
        normalize_image_details(&mut input, model_info);
        // when using the *Freeform* apply_patch tool specifically, tool outputs
        // should be structured text, not json. Do NOT reserialize when using
        // the Function tool - note that this differs from the check above for
        // instructions. We declare the result as a named variable for clarity.
        let is_freeform_apply_patch_tool_present = self.tools.iter().any(|tool| match tool {
            ToolSpec::Freeform(f) => f.name == "apply_patch",
            _ => false,
        });
        if is_freeform_apply_patch_tool_present {
            reserialize_shell_outputs(&mut input);
        }

        input
    }
}

fn normalize_image_details(items: &mut [ResponseItem], model_info: &ModelInfo) {
    for item in items {
        match item {
            ResponseItem::Message { content, .. } => {
                for content_item in content {
                    if let ContentItem::InputImage { detail, .. } = content_item {
                        normalize_image_detail(detail, model_info);
                    }
                }
            }
            ResponseItem::FunctionCallOutput { output, .. }
            | ResponseItem::CustomToolCallOutput { output, .. } => {
                if let Some(content) = output.content_items_mut() {
                    for content_item in content {
                        if let FunctionCallOutputContentItem::InputImage { detail, .. } =
                            content_item
                        {
                            normalize_image_detail(detail, model_info);
                        }
                    }
                }
            }
            ResponseItem::AdditionalTools { .. }
            | ResponseItem::Reasoning { .. }
            | ResponseItem::AgentMessage { .. }
            | ResponseItem::LocalShellCall { .. }
            | ResponseItem::FunctionCall { .. }
            | ResponseItem::ToolSearchCall { .. }
            | ResponseItem::CustomToolCall { .. }
            | ResponseItem::ToolSearchOutput { .. }
            | ResponseItem::WebSearchCall { .. }
            | ResponseItem::ImageGenerationCall { .. }
            | ResponseItem::Compaction { .. }
            | ResponseItem::ConfigurationUpdate { .. }
            | ResponseItem::CompactionTrigger { .. }
            | ResponseItem::ContextCompaction { .. }
            | ResponseItem::Other => {}
        }
    }
}

fn reserialize_shell_outputs(items: &mut [ResponseItem]) {
    let mut shell_call_ids: HashSet<String> = HashSet::new();

    items.iter_mut().for_each(|item| match item {
        ResponseItem::LocalShellCall { call_id, id, .. } => {
            if let Some(identifier) = call_id.clone().or_else(|| id.clone().map(String::from)) {
                shell_call_ids.insert(identifier);
            }
        }
        ResponseItem::CustomToolCall {
            id: _,
            status: _,
            call_id,
            name,
            input: _,
            ..
        } if name == "apply_patch" => {
            shell_call_ids.insert(call_id.clone());
        }
        ResponseItem::FunctionCall { name, call_id, .. }
            if is_shell_tool_name(name) || name == "apply_patch" =>
        {
            shell_call_ids.insert(call_id.clone());
        }
        ResponseItem::FunctionCallOutput {
            call_id, output, ..
        } => {
            if let Some(call_id) = call_id
                && shell_call_ids.remove(call_id)
                && let Some(structured) = output
                    .text_content()
                    .and_then(parse_structured_shell_output)
            {
                output.body = FunctionCallOutputBody::Text(structured);
            }
        }
        ResponseItem::CustomToolCallOutput {
            call_id, output, ..
        } => {
            if shell_call_ids.remove(call_id)
                && let Some(structured) = output
                    .text_content()
                    .and_then(parse_structured_shell_output)
            {
                output.body = FunctionCallOutputBody::Text(structured);
            }
        }
        _ => {}
    })
}

fn is_shell_tool_name(name: &str) -> bool {
    name == "shell"
}

#[derive(Deserialize)]
struct ExecOutputJson {
    output: String,
    metadata: ExecOutputMetadataJson,
}

#[derive(Deserialize)]
struct ExecOutputMetadataJson {
    exit_code: i32,
    duration_seconds: f32,
}

fn parse_structured_shell_output(raw: &str) -> Option<String> {
    let parsed: ExecOutputJson = serde_json::from_str(raw).ok()?;
    Some(build_structured_output(&parsed))
}

fn build_structured_output(parsed: &ExecOutputJson) -> String {
    let mut sections = Vec::new();
    sections.push(format!("Exit code: {}", parsed.metadata.exit_code));
    sections.push(format!(
        "Wall time: {} seconds",
        parsed.metadata.duration_seconds
    ));

    let mut output = parsed.output.clone();
    if let Some((stripped, total_lines)) = strip_total_output_header(&parsed.output) {
        sections.push(format!("Total output lines: {total_lines}"));
        output = stripped.to_string();
    }

    sections.push("Output:".to_string());
    sections.push(output);

    sections.join("\n")
}

fn strip_total_output_header(output: &str) -> Option<(&str, u32)> {
    let after_prefix = output.strip_prefix("Total output lines: ")?;
    let (total_segment, remainder) = after_prefix.split_once('\n')?;
    let total_lines = total_segment.parse::<u32>().ok()?;
    let remainder = remainder.strip_prefix('\n').unwrap_or(remainder);
    Some((remainder, total_lines))
}

fn normalize_image_detail(detail: &mut Option<ImageDetail>, model_info: &ModelInfo) {
    if model_info.use_responses_lite {
        *detail = None;
    } else if *detail == Some(ImageDetail::Original) && !model_info.supports_image_detail_original {
        *detail = Some(DEFAULT_IMAGE_DETAIL);
    }
}

pub struct ResponseStream {
    pub(crate) rx_event: mpsc::Receiver<Result<ResponseEvent>>,
    /// Signals the mapper task that the consumer stopped polling before the
    /// provider stream reached its own terminal event.
    pub(crate) consumer_dropped: CancellationToken,
}

impl Stream for ResponseStream {
    type Item = Result<ResponseEvent>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.rx_event.poll_recv(cx)
    }
}

impl Drop for ResponseStream {
    fn drop(&mut self) {
        self.consumer_dropped.cancel();
    }
}

#[cfg(test)]
#[path = "client_common_tests.rs"]
mod tests;
