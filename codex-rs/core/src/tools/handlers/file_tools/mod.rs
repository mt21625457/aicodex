use crate::function_tool::FunctionCallError;
use crate::session::step_context::StepContext;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::context::ToolPayload;
use crate::tools::context::boxed_tool_output;
use crate::tools::handlers::parse_arguments;
use crate::tools::handlers::resolve_tool_environment;
use crate::tools::hook_names::HookToolName;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::PostToolUsePayload;
use crate::tools::registry::PreToolUsePayload;
use crate::tools::registry::ToolExecutor;
use codex_file_system::ConditionalWritePrecondition;
use codex_tools::JsonSchema;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolName;
use codex_tools::ToolSpec;
use codex_utils_path_uri::PathUri;
use sha2::Digest;
use sha2::Sha256;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

const MAX_FILE_BYTES: usize = 8 * 1024 * 1024;
const MAX_SCAN_BYTES: usize = 64 * 1024 * 1024;
const MAX_OUTPUT_BYTES: usize = 64 * 1024;
// Repository output accounting approximates one token as four UTF-8 bytes.
const MAX_OUTPUT_TOKEN_BYTES: usize = 10_000 * 4;
const MAX_LINES: usize = 2_000;
const MAX_PATH_BYTES: usize = 4_096;
const MAX_MUTATION_ARGUMENT_BYTES: usize = 64 * 1024;
const MAX_MUTATION_TOKEN_BYTES: usize = 10_000 * 4;
const MAX_RECEIPTS: usize = 128;
const MAX_RECEIPT_RANGES_PER_ENTRY: usize = 64;
const MAX_RECEIPT_RANGES: usize = 1_024;
const MAX_RECEIPT_BYTES: usize = 256 * 1024;
const OUTPUT_TRUNCATION_MARKER: &str = "[output truncated at line boundary]";

#[derive(Debug, Default)]
pub(crate) struct FileToolState {
    receipts: HashMap<FileReceiptKey, FileReceipt>,
    next_access: u64,
    total_ranges: usize,
    total_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct FileReceiptKey {
    environment_id: String,
    path: PathUri,
}

#[derive(Debug, Clone)]
struct FileReceipt {
    fingerprint: [u8; 32],
    file_size: u64,
    modified_at_ms: i64,
    full_coverage: bool,
    write_eligible: bool,
    observed_ranges: Vec<(usize, usize)>,
    step_id: u64,
    last_used: u64,
    accounted_bytes: usize,
}

pub(crate) struct ReadFileHandler {
    state: Arc<Mutex<FileToolState>>,
    multi_environment: bool,
}

pub(crate) struct EditFileHandler {
    state: Arc<Mutex<FileToolState>>,
    multi_environment: bool,
}

pub(crate) struct WriteFileHandler {
    state: Arc<Mutex<FileToolState>>,
    multi_environment: bool,
}

impl ReadFileHandler {
    pub(crate) fn new(state: Arc<Mutex<FileToolState>>, multi_environment: bool) -> Self {
        Self {
            state,
            multi_environment,
        }
    }
}

impl EditFileHandler {
    pub(crate) fn new(state: Arc<Mutex<FileToolState>>, multi_environment: bool) -> Self {
        Self {
            state,
            multi_environment,
        }
    }
}

impl WriteFileHandler {
    pub(crate) fn new(state: Arc<Mutex<FileToolState>>, multi_environment: bool) -> Self {
        Self {
            state,
            multi_environment,
        }
    }
}

impl ToolExecutor<ToolInvocation> for ReadFileHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain("read_file")
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::Function(ResponsesApiTool {
            name: "read_file".to_string(),
            description: "Read a bounded text file through Codex's filesystem layer.".to_string(),
            strict: false,
            defer_loading: None,
            parameters: file_schema(
                [
                    (
                        "path",
                        JsonSchema::string(Some("Workspace-relative file path.".to_string())),
                    ),
                    (
                        "offset",
                        JsonSchema::integer(Some("1-based first line.".to_string())),
                    ),
                    (
                        "limit",
                        JsonSchema::integer(Some("Maximum number of lines.".to_string())),
                    ),
                    (
                        "environment_id",
                        JsonSchema::string(Some("Target environment id.".to_string())),
                    ),
                ],
                vec!["path".to_string()],
            ),
            output_schema: None,
        })
    }

    fn supports_parallel_tool_calls(&self) -> bool {
        true
    }

    fn handle(&self, invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'_> {
        Box::pin(read_file(
            self.state.clone(),
            self.multi_environment,
            invocation,
        ))
    }
}

impl CoreToolRuntime for ReadFileHandler {
    fn pre_tool_use_payload(&self, invocation: &ToolInvocation) -> Option<PreToolUsePayload> {
        file_tool_pre_payload("read_file", invocation)
    }

    fn with_updated_hook_input(
        &self,
        invocation: ToolInvocation,
        updated_input: serde_json::Value,
    ) -> Result<ToolInvocation, FunctionCallError> {
        updated_file_tool_invocation(invocation, updated_input)
    }

    fn post_tool_use_payload(
        &self,
        invocation: &ToolInvocation,
        result: &dyn ToolOutput,
    ) -> Option<PostToolUsePayload> {
        file_tool_post_payload("read_file", invocation, result)
    }
}

impl ToolExecutor<ToolInvocation> for EditFileHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain("edit_file")
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::Function(ResponsesApiTool {
            name: "edit_file".to_string(),
            description: "Replace exact text in a previously read text file.".to_string(),
            strict: false,
            defer_loading: None,
            parameters: file_schema(
                [
                    (
                        "path",
                        JsonSchema::string(Some("Workspace-relative file path.".to_string())),
                    ),
                    (
                        "old_string",
                        JsonSchema::string(Some("Exact text to replace.".to_string())),
                    ),
                    (
                        "new_string",
                        JsonSchema::string(Some("Replacement text.".to_string())),
                    ),
                    (
                        "replace_all",
                        JsonSchema::boolean(Some("Replace every match.".to_string())),
                    ),
                    (
                        "environment_id",
                        JsonSchema::string(Some("Target environment id.".to_string())),
                    ),
                ],
                vec![
                    "path".to_string(),
                    "old_string".to_string(),
                    "new_string".to_string(),
                ],
            ),
            output_schema: None,
        })
    }

    fn handle(&self, invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'_> {
        Box::pin(edit_file(
            self.state.clone(),
            self.multi_environment,
            invocation,
        ))
    }
}

impl CoreToolRuntime for EditFileHandler {
    fn pre_tool_use_payload(&self, invocation: &ToolInvocation) -> Option<PreToolUsePayload> {
        file_tool_pre_payload("edit_file", invocation)
    }

    fn with_updated_hook_input(
        &self,
        invocation: ToolInvocation,
        updated_input: serde_json::Value,
    ) -> Result<ToolInvocation, FunctionCallError> {
        updated_file_tool_invocation(invocation, updated_input)
    }

    fn post_tool_use_payload(
        &self,
        invocation: &ToolInvocation,
        result: &dyn ToolOutput,
    ) -> Option<PostToolUsePayload> {
        file_tool_post_payload("edit_file", invocation, result)
    }
}

impl ToolExecutor<ToolInvocation> for WriteFileHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain("write_file")
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::Function(ResponsesApiTool {
            name: "write_file".to_string(),
            description:
                "Create or overwrite a bounded text file through Codex's filesystem layer."
                    .to_string(),
            strict: false,
            defer_loading: None,
            parameters: file_schema(
                [
                    (
                        "path",
                        JsonSchema::string(Some("Workspace-relative file path.".to_string())),
                    ),
                    (
                        "content",
                        JsonSchema::string(Some("Complete UTF-8 file content.".to_string())),
                    ),
                    (
                        "environment_id",
                        JsonSchema::string(Some("Target environment id.".to_string())),
                    ),
                ],
                vec!["path".to_string(), "content".to_string()],
            ),
            output_schema: None,
        })
    }

    fn handle(&self, invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'_> {
        Box::pin(write_file(
            self.state.clone(),
            self.multi_environment,
            invocation,
        ))
    }
}

impl CoreToolRuntime for WriteFileHandler {
    fn pre_tool_use_payload(&self, invocation: &ToolInvocation) -> Option<PreToolUsePayload> {
        file_tool_pre_payload("write_file", invocation)
    }

    fn with_updated_hook_input(
        &self,
        invocation: ToolInvocation,
        updated_input: serde_json::Value,
    ) -> Result<ToolInvocation, FunctionCallError> {
        updated_file_tool_invocation(invocation, updated_input)
    }

    fn post_tool_use_payload(
        &self,
        invocation: &ToolInvocation,
        result: &dyn ToolOutput,
    ) -> Option<PostToolUsePayload> {
        file_tool_post_payload("write_file", invocation, result)
    }
}

fn file_tool_input(payload: &ToolPayload) -> Option<serde_json::Value> {
    let ToolPayload::Function { arguments } = payload else {
        return None;
    };
    serde_json::from_str(arguments).ok()
}

fn file_tool_pre_payload(
    tool_name: &str,
    invocation: &ToolInvocation,
) -> Option<PreToolUsePayload> {
    Some(PreToolUsePayload {
        tool_name: HookToolName::new(tool_name),
        tool_input: file_tool_input(&invocation.payload)?,
    })
}

fn updated_file_tool_invocation(
    mut invocation: ToolInvocation,
    updated_input: serde_json::Value,
) -> Result<ToolInvocation, FunctionCallError> {
    if !updated_input.is_object() {
        return Err(FunctionCallError::RespondToModel(
            "file tool hook output must be a JSON object".to_string(),
        ));
    }
    invocation.payload = ToolPayload::Function {
        arguments: updated_input.to_string(),
    };
    Ok(invocation)
}

fn file_tool_post_payload(
    tool_name: &str,
    invocation: &ToolInvocation,
    result: &dyn ToolOutput,
) -> Option<PostToolUsePayload> {
    let tool_response = result.post_tool_use_response(&invocation.call_id, &invocation.payload)?;
    Some(PostToolUsePayload {
        tool_name: HookToolName::new(tool_name),
        tool_use_id: invocation.call_id.clone(),
        tool_input: file_tool_input(&invocation.payload)?,
        tool_response,
    })
}

mod io;
mod mutation;
mod receipt;
mod runtime;
mod schema;
mod text;
use runtime::edit_file;
use runtime::read_file;
use runtime::write_file;
use schema::file_schema;
