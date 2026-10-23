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
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

/// API request payload for a single model turn
#[derive(Debug, Clone)]
pub struct Prompt {
    /// Conversation context input items.
    pub input: Vec<ResponseItem>,

    /// Tool definitions to inject into this request, empty when supplied by history.
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
    let mut remaining_admissions = shell_output_cache_admission_budget(items);
    reserialize_shell_outputs_with_cache(
        items,
        Some(&SHELL_OUTPUT_CACHE),
        &mut remaining_admissions,
    );
}

/// 先用正文长度计算上界；可能超过容量时再以借用的 call_id 核对真实配对，
/// 避免无关工具或重复结果把小 shell 工作集误判为大集合。不解析或散列正文。
/// 大历史每次最多接纳一个 miss，避免顺序重放逐项驱逐后面仍将使用的缓存；
/// 保留一次接纳让新内容仍能进入缓存，重复正文也能在同一请求中立即复用。
/// 小历史继续使用原 LRU；全局容量不变，也不创建随并发请求增长的结果快照。
fn shell_output_cache_admission_budget(items: &[ResponseItem]) -> usize {
    if !shell_output_cache_runtime_available() {
        return 0;
    }
    let candidates = items
        .iter()
        .filter_map(|item| match item {
            ResponseItem::FunctionCallOutput { output, .. }
            | ResponseItem::CustomToolCallOutput { output, .. } => output.text_content(),
            _ => None,
        })
        .filter(|raw| shell_output_cache_size_eligible(raw.len()))
        .take(SHELL_OUTPUT_CACHE_CAPACITY + 1)
        .count();
    if candidates <= SHELL_OUTPUT_CACHE_CAPACITY {
        return usize::MAX;
    }
    let mut pending = HashSet::new();
    let mut paired_candidates = 0;
    for item in items {
        if let Some(call_id) = shell_call_id(item) {
            pending.insert(call_id);
            continue;
        }
        let (call_id, output) = match item {
            ResponseItem::FunctionCallOutput {
                call_id: Some(call_id),
                output,
                ..
            }
            | ResponseItem::CustomToolCallOutput {
                call_id, output, ..
            } => (call_id, output),
            _ => continue,
        };
        // 与转换路径一样，先消费配对；非文本/小结果也不能让后续重复结果再次匹配。
        if pending.remove(call_id.as_str())
            && output
                .text_content()
                .is_some_and(|raw| shell_output_cache_size_eligible(raw.len()))
        {
            paired_candidates += 1;
            if paired_candidates > SHELL_OUTPUT_CACHE_CAPACITY {
                return 1;
            }
        }
    }
    usize::MAX
}

fn shell_call_id(item: &ResponseItem) -> Option<&str> {
    match item {
        ResponseItem::LocalShellCall { call_id, id, .. } => {
            call_id.as_deref().or_else(|| id.as_deref())
        }
        ResponseItem::CustomToolCall { call_id, name, .. } if name == "apply_patch" => {
            Some(call_id)
        }
        ResponseItem::FunctionCall { name, call_id, .. }
            if is_shell_tool_name(name) || name == "apply_patch" =>
        {
            Some(call_id)
        }
        _ => None,
    }
}

fn reserialize_shell_outputs_with_cache(
    items: &mut [ResponseItem],
    cache: Option<&ShellOutputCache>,
    remaining_admissions: &mut usize,
) {
    let mut shell_call_ids: HashSet<String> = HashSet::new();

    items.iter_mut().for_each(|item| {
        if let Some(call_id) = shell_call_id(item) {
            shell_call_ids.insert(call_id.to_owned());
            return;
        }
        match item {
            ResponseItem::FunctionCallOutput {
                call_id, output, ..
            } => {
                if let Some(call_id) = call_id
                    && shell_call_ids.remove(call_id)
                    && let Some(structured) = output.text_content().and_then(|raw| {
                        parse_structured_shell_output_with_cache(raw, cache, remaining_admissions)
                    })
                {
                    output.body = FunctionCallOutputBody::Text(structured);
                }
            }
            ResponseItem::CustomToolCallOutput {
                call_id, output, ..
            } => {
                if shell_call_ids.remove(call_id)
                    && let Some(structured) = output.text_content().and_then(|raw| {
                        parse_structured_shell_output_with_cache(raw, cache, remaining_admissions)
                    })
                {
                    output.body = FunctionCallOutputBody::Text(structured);
                }
            }
            _ => {}
        }
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

const SHELL_OUTPUT_CACHE_CAPACITY: usize = 16;
const SHELL_OUTPUT_CACHE_MIN_BYTES: usize = 4096;
const SHELL_OUTPUT_CACHE_MAX_BYTES: usize = 1024 * 1024;
type ShellOutputCache = codex_utils_cache::BlockingLruCache<String, Option<Arc<str>>>;
static SHELL_OUTPUT_CACHE: std::sync::LazyLock<ShellOutputCache> = std::sync::LazyLock::new(|| {
    codex_utils_cache::BlockingLruCache::new(
        std::num::NonZeroUsize::MIN.saturating_add(SHELL_OUTPUT_CACHE_CAPACITY - 1),
    )
});

fn shell_output_cache_size_eligible(bytes: usize) -> bool {
    (SHELL_OUTPUT_CACHE_MIN_BYTES..=SHELL_OUTPUT_CACHE_MAX_BYTES).contains(&bytes)
}

fn shell_output_cache_runtime_available() -> bool {
    tokio::runtime::Handle::try_current().is_ok_and(|handle| {
        matches!(
            handle.runtime_flavor(),
            tokio::runtime::RuntimeFlavor::MultiThread
        )
    })
}

#[cfg(test)]
thread_local! {
    static SHELL_OUTPUT_PARSE_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
fn parse_structured_shell_output(raw: &str) -> Option<String> {
    let mut remaining_admissions = usize::MAX;
    parse_structured_shell_output_with_cache(
        raw,
        Some(&SHELL_OUTPUT_CACHE),
        &mut remaining_admissions,
    )
}

fn parse_structured_shell_output_with_cache(
    raw: &str,
    cache: Option<&ShellOutputCache>,
    remaining_admissions: &mut usize,
) -> Option<String> {
    // 只缓存纯文本转换，键仍为完整原文。配对和输出顺序由调用方保持，持久化历史不变。
    // 最多 16 项、每项原文与结果各 1 MiB；小输出及非多线程 Tokio 路径不使用缓存。
    let cache = cache.filter(|_| {
        shell_output_cache_size_eligible(raw.len()) && shell_output_cache_runtime_available()
    });
    if let Some(cache) = cache
        && let Some(cached) = cache.get(raw)
    {
        // 外层 None 是 miss，内层 None 是已缓存的解析失败，不能混淆负缓存。
        return cached.map(|text| text.to_string());
    }
    #[cfg(test)]
    SHELL_OUTPUT_PARSE_COUNT.with(|count| count.set(count.get() + 1));
    let result = serde_json::from_str::<ExecOutputJson>(raw)
        .ok()
        .map(|parsed| build_structured_output(&parsed));
    if let Some(cache) = cache
        && *remaining_admissions > 0
        && result
            .as_ref()
            .is_none_or(|text| text.len() <= SHELL_OUTPUT_CACHE_MAX_BYTES)
    {
        *remaining_admissions -= 1;
        cache.insert(raw.to_owned(), result.as_deref().map(Arc::from));
    }
    result
}

fn build_structured_output(parsed: &ExecOutputJson) -> String {
    let mut formatted = format!(
        "Exit code: {}\nWall time: {} seconds\n",
        parsed.metadata.exit_code, parsed.metadata.duration_seconds
    );

    // 正文只借用原字符串；保留原来的头部识别与换行规则，最终仅复制进返回值一次。
    let output = if let Some((stripped, total_lines)) = strip_total_output_header(&parsed.output) {
        formatted.push_str(&format!("Total output lines: {total_lines}\n"));
        stripped
    } else {
        parsed.output.as_str()
    };
    formatted.reserve("Output:\n".len() + output.len());
    formatted.push_str("Output:\n");
    formatted.push_str(output);
    formatted
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
    pub(crate) interrupt: Option<oneshot::Sender<codex_api::ResponseInterrupt>>,
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
