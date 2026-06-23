use std::path::Path;

use codex_exec_server::ExecutorFileSystem;
use codex_protocol::models::FunctionCallOutputContentItem;
use codex_tools::CLAUDE_TEXT_EDITOR_TOOL_NAME;
use codex_tools::JsonSchema;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolName;
use codex_tools::ToolSpec;
use codex_utils_absolute_path::AbsolutePathBuf;
use codex_utils_path_uri::PathUri;
use serde::Deserialize;

use crate::function_tool::FunctionCallError;
use crate::session::turn_context::TurnEnvironment;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::context::ToolPayload;
use crate::tools::context::boxed_tool_output;
use crate::tools::handlers::ApplyPatchHandler;
use crate::tools::handlers::parse_arguments;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;

/// Executes Anthropic's native `text_editor` schema through Codex-controlled
/// file reads and apply_patch mutations.
pub(crate) struct ClaudeTextEditorHandler {
    multi_environment: bool,
}

impl ClaudeTextEditorHandler {
    pub(crate) fn new(multi_environment: bool) -> Self {
        Self { multi_environment }
    }
}

#[derive(Debug, Deserialize)]
struct ClaudeTextEditorArgs {
    command: ClaudeTextEditorCommand,
    path: String,
    #[serde(default)]
    file_text: Option<String>,
    #[serde(default)]
    old_str: Option<String>,
    #[serde(default)]
    new_str: Option<String>,
    #[serde(default)]
    insert_line: Option<usize>,
    #[serde(default)]
    view_range: Option<Vec<i64>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ClaudeTextEditorCommand {
    View,
    Create,
    StrReplace,
    Insert,
    UndoEdit,
}

struct ResolvedTextEditorPath {
    absolute: AbsolutePathBuf,
    patch_path: String,
}

impl ToolExecutor<ToolInvocation> for ClaudeTextEditorHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(CLAUDE_TEXT_EDITOR_TOOL_NAME)
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::Function(ResponsesApiTool {
            name: CLAUDE_TEXT_EDITOR_TOOL_NAME.to_string(),
            description: "Claude native text editor runtime.".to_string(),
            strict: false,
            defer_loading: None,
            parameters: JsonSchema::default(),
            output_schema: None,
        })
    }

    fn handle(&self, invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'_> {
        Box::pin(async move {
            let turn = invocation.turn.clone();
            let arguments = match &invocation.payload {
                ToolPayload::Function { arguments } => arguments.clone(),
                _ => {
                    return Err(FunctionCallError::RespondToModel(
                        "Claude text editor received unsupported payload".to_string(),
                    ));
                }
            };
            let args: ClaudeTextEditorArgs = parse_arguments(&arguments)?;
            let Some(turn_environment) = turn.environments.primary() else {
                return Err(FunctionCallError::RespondToModel(
                    "Claude text editor is unavailable in this session".to_string(),
                ));
            };
            let path = resolve_text_editor_path(turn_environment, &args.path)?;
            let fs = turn_environment.environment.get_filesystem();
            let sandbox = turn.file_system_sandbox_context(
                /*additional_permissions*/ None,
                turn_environment.cwd(),
            );

            match args.command {
                ClaudeTextEditorCommand::View => {
                    view_path(
                        fs.as_ref(),
                        &path.absolute,
                        args.view_range.as_deref(),
                        Some(&sandbox),
                    )
                    .await
                }
                ClaudeTextEditorCommand::Create => {
                    let file_text = required_arg(args.file_text, "file_text", "create")?;
                    apply_generated_patch(
                        invocation,
                        self.multi_environment,
                        create_file_patch(&path.patch_path, &file_text),
                    )
                    .await
                }
                ClaudeTextEditorCommand::StrReplace => {
                    let old_str = required_arg(args.old_str, "old_str", "str_replace")?;
                    let new_str = required_arg(args.new_str, "new_str", "str_replace")?;
                    let path_uri = PathUri::from_abs_path(&path.absolute);
                    let current = fs
                        .read_file_text(&path_uri, Some(&sandbox))
                        .await
                        .map_err(text_editor_io_error)?;
                    let matches = current.matches(&old_str).count();
                    if matches != 1 {
                        return Err(FunctionCallError::RespondToModel(format!(
                            "Claude text editor str_replace expected exactly one match, found {matches}"
                        )));
                    }
                    let updated = current.replacen(&old_str, &new_str, 1);
                    apply_generated_patch(
                        invocation,
                        self.multi_environment,
                        update_file_patch(&path.patch_path, &current, &updated),
                    )
                    .await
                }
                ClaudeTextEditorCommand::Insert => {
                    let insert_line = args.insert_line.ok_or_else(|| {
                        FunctionCallError::RespondToModel(
                            "Claude text editor insert requires `insert_line`".to_string(),
                        )
                    })?;
                    let new_str = required_arg(args.new_str, "new_str", "insert")?;
                    let path_uri = PathUri::from_abs_path(&path.absolute);
                    let current = fs
                        .read_file_text(&path_uri, Some(&sandbox))
                        .await
                        .map_err(text_editor_io_error)?;
                    let updated = insert_after_line(&current, insert_line, &new_str)?;
                    apply_generated_patch(
                        invocation,
                        self.multi_environment,
                        update_file_patch(&path.patch_path, &current, &updated),
                    )
                    .await
                }
                ClaudeTextEditorCommand::UndoEdit => Err(FunctionCallError::RespondToModel(
                    "Claude text editor undo_edit is not available for the active native text editor tool version"
                        .to_string(),
                )),
            }
            .map(boxed_tool_output)
        })
    }
}

impl CoreToolRuntime for ClaudeTextEditorHandler {
    fn matches_kind(&self, payload: &ToolPayload) -> bool {
        matches!(payload, ToolPayload::Function { .. })
    }
}

fn resolve_text_editor_path(
    turn_environment: &TurnEnvironment,
    raw_path: &str,
) -> Result<ResolvedTextEditorPath, FunctionCallError> {
    let trimmed = raw_path.trim();
    if trimmed.is_empty() {
        return Err(FunctionCallError::RespondToModel(
            "Claude text editor path must not be empty".to_string(),
        ));
    }
    let cwd = turn_environment.cwd().to_abs_path().map_err(|_| {
        FunctionCallError::RespondToModel(
            "Claude text editor is unavailable for this workspace path".to_string(),
        )
    })?;
    let absolute = AbsolutePathBuf::resolve_path_against_base(Path::new(trimmed), cwd.as_path());
    let relative = absolute
        .as_path()
        .strip_prefix(cwd.as_path())
        .map_err(|_| {
            FunctionCallError::RespondToModel(
                "Claude text editor path must stay inside the current workspace".to_string(),
            )
        })?;
    let patch_path = relative_path_for_patch(relative)?;
    Ok(ResolvedTextEditorPath {
        absolute,
        patch_path,
    })
}

fn relative_path_for_patch(path: &Path) -> Result<String, FunctionCallError> {
    if path.as_os_str().is_empty() {
        return Err(FunctionCallError::RespondToModel(
            "Claude text editor path must point to a file or directory under the workspace"
                .to_string(),
        ));
    }
    let value = path.to_string_lossy().replace('\\', "/");
    if value.split('/').any(|part| part == "..") {
        return Err(FunctionCallError::RespondToModel(
            "Claude text editor path traversal is not allowed".to_string(),
        ));
    }
    Ok(value)
}

async fn view_path(
    fs: &dyn ExecutorFileSystem,
    path: &AbsolutePathBuf,
    view_range: Option<&[i64]>,
    sandbox: Option<&codex_exec_server::FileSystemSandboxContext>,
) -> Result<FunctionToolOutput, FunctionCallError> {
    let path_uri = PathUri::from_abs_path(path);
    let metadata = fs
        .get_metadata(&path_uri, sandbox)
        .await
        .map_err(text_editor_io_error)?;
    if metadata.is_directory {
        let mut entries = fs
            .read_directory(&path_uri, sandbox)
            .await
            .map_err(text_editor_io_error)?;
        entries.sort_by(|left, right| left.file_name.cmp(&right.file_name));
        let listing = entries
            .into_iter()
            .map(|entry| {
                let suffix = if entry.is_directory { "/" } else { "" };
                format!("{}{}", entry.file_name, suffix)
            })
            .collect::<Vec<_>>()
            .join("\n");
        return Ok(FunctionToolOutput::from_text(listing, Some(true)));
    }
    if !metadata.is_file {
        return Err(FunctionCallError::RespondToModel(
            "Claude text editor can only view files or directories".to_string(),
        ));
    }
    let text = fs
        .read_file_text(&path_uri, sandbox)
        .await
        .map_err(text_editor_io_error)?;
    Ok(FunctionToolOutput::from_text(
        line_numbered_view(&text, view_range)?,
        Some(true),
    ))
}

fn line_numbered_view(text: &str, view_range: Option<&[i64]>) -> Result<String, FunctionCallError> {
    let lines = text.lines().collect::<Vec<_>>();
    let (start, end) = match view_range {
        Some([start, end]) if *start > 0 && (*end == -1 || *end >= *start) => {
            let start = *start as usize;
            let end = if *end == -1 {
                lines.len()
            } else {
                (*end as usize).min(lines.len())
            };
            (start, end)
        }
        Some(_) => {
            return Err(FunctionCallError::RespondToModel(
                "Claude text editor view_range must be [start_line, end_line] with positive line numbers, or end_line -1"
                    .to_string(),
            ));
        }
        None => (1, lines.len()),
    };
    Ok(lines
        .iter()
        .enumerate()
        .skip(start.saturating_sub(1))
        .take(end.saturating_sub(start).saturating_add(1))
        .map(|(index, line)| format!("{:>6}\t{}", index + 1, line))
        .collect::<Vec<_>>()
        .join("\n"))
}

async fn apply_generated_patch(
    invocation: ToolInvocation,
    multi_environment: bool,
    patch: String,
) -> Result<FunctionToolOutput, FunctionCallError> {
    let mut invocation = invocation;
    invocation.tool_name = ToolName::plain("apply_patch");
    invocation.payload = ToolPayload::Custom { input: patch };
    let call_id = invocation.call_id.clone();
    let payload = invocation.payload.clone();
    let output = ApplyPatchHandler::new(multi_environment)
        .handle(invocation)
        .await?;
    let text = output
        .post_tool_use_response(&call_id, &payload)
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_else(|| output.log_preview());
    Ok(FunctionToolOutput {
        body: vec![FunctionCallOutputContentItem::InputText { text }],
        success: Some(true),
        post_tool_use_response: None,
    })
}

fn create_file_patch(path: &str, file_text: &str) -> String {
    let mut patch = format!("*** Begin Patch\n*** Add File: {path}\n");
    append_patch_lines(&mut patch, '+', file_text);
    patch.push_str("*** End Patch");
    patch
}

fn update_file_patch(path: &str, current: &str, updated: &str) -> String {
    let mut patch = format!("*** Begin Patch\n*** Update File: {path}\n@@\n");
    append_patch_lines(&mut patch, '-', current);
    append_patch_lines(&mut patch, '+', updated);
    patch.push_str("*** End Patch");
    patch
}

fn append_patch_lines(patch: &mut String, prefix: char, text: &str) {
    if text.is_empty() {
        return;
    }
    for line in text.split_inclusive('\n') {
        patch.push(prefix);
        patch.push_str(line.strip_suffix('\n').unwrap_or(line));
        patch.push('\n');
    }
}

fn insert_after_line(
    current: &str,
    insert_line: usize,
    new_str: &str,
) -> Result<String, FunctionCallError> {
    let mut lines = current.split_inclusive('\n').collect::<Vec<_>>();
    if current.is_empty() {
        lines.clear();
    }
    if insert_line > lines.len() {
        return Err(FunctionCallError::RespondToModel(format!(
            "Claude text editor insert_line {insert_line} exceeds file line count {}",
            lines.len()
        )));
    }
    let mut updated = String::new();
    for line in lines.iter().take(insert_line) {
        updated.push_str(line);
    }
    updated.push_str(new_str);
    if !new_str.ends_with('\n') {
        updated.push('\n');
    }
    for line in lines.iter().skip(insert_line) {
        updated.push_str(line);
    }
    Ok(updated)
}

fn required_arg(
    value: Option<String>,
    field: &str,
    command: &str,
) -> Result<String, FunctionCallError> {
    value.ok_or_else(|| {
        FunctionCallError::RespondToModel(format!(
            "Claude text editor {command} requires `{field}`"
        ))
    })
}

fn text_editor_io_error(error: std::io::Error) -> FunctionCallError {
    FunctionCallError::RespondToModel(format!("Claude text editor filesystem error: {error}"))
}
