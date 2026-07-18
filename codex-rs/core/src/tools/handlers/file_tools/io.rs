use super::*;
use crate::function_tool::FunctionCallError;
use codex_file_system::FileMetadata;
use futures::StreamExt;

pub(super) async fn read_file_prefix(
    fs: &dyn codex_file_system::ExecutorFileSystem,
    path: &PathUri,
    sandbox: &codex_file_system::FileSystemSandboxContext,
    max_bytes: usize,
) -> Result<(Vec<u8>, bool), FunctionCallError> {
    let mut stream = fs
        .read_file_stream(path, Some(sandbox))
        .await
        .map_err(file_io_error)?;
    let mut bytes = Vec::with_capacity(max_bytes.min(MAX_FILE_BYTES));
    loop {
        let Some(chunk) = stream.next().await else {
            return Ok((bytes, true));
        };
        let remaining = max_bytes.saturating_sub(bytes.len());
        if chunk.as_ref().is_ok_and(|chunk| chunk.len() > remaining) {
            let chunk = chunk.map_err(file_io_error)?;
            bytes.extend_from_slice(&chunk[..remaining]);
            return Ok((bytes, false));
        }
        let chunk = chunk.map_err(file_io_error)?;
        if remaining == 0 {
            return Ok((bytes, false));
        }
        bytes.extend_from_slice(&chunk);
    }
}

pub(super) async fn read_editable_file(
    fs: &dyn codex_file_system::ExecutorFileSystem,
    path: &PathUri,
    sandbox: &codex_file_system::FileSystemSandboxContext,
    tool_name: &str,
) -> Result<(Vec<u8>, FileMetadata), FunctionCallError> {
    let metadata = fs
        .get_metadata(path, Some(sandbox))
        .await
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound && tool_name == "edit_file" {
                FunctionCallError::RespondToModel(
                    "edit_file cannot create a missing file; use write_file to create it"
                        .to_string(),
                )
            } else {
                file_io_error(error)
            }
        })?;
    if metadata.is_directory || !metadata.is_file {
        return Err(FunctionCallError::RespondToModel(format!(
            "{tool_name} supports regular text files only"
        )));
    }
    if metadata.size > MAX_FILE_BYTES as u64 {
        return Err(FunctionCallError::RespondToModel(format!(
            "{tool_name} supports files up to the 8 MiB editable limit; use a specialized script"
        )));
    }
    let (bytes, reached_eof) = read_file_prefix(fs, path, sandbox, MAX_FILE_BYTES).await?;
    if !reached_eof {
        return Err(FunctionCallError::RespondToModel(format!(
            "{tool_name} supports files up to the 8 MiB editable limit; use a specialized script"
        )));
    }
    let metadata = fs
        .get_metadata(path, Some(sandbox))
        .await
        .map_err(file_io_error)?;
    Ok((bytes, metadata))
}

pub(super) async fn resolve_file_target<'a>(
    step_context: &'a StepContext,
    turn: &'a crate::session::turn_context::TurnContext,
    environment_id: Option<&str>,
    raw_path: &str,
    multi_environment: bool,
) -> Result<
    (
        &'a crate::session::turn_context::TurnEnvironment,
        PathUri,
        PathUri,
        codex_file_system::FileSystemSandboxContext,
    ),
    FunctionCallError,
> {
    if raw_path.trim().is_empty() || raw_path.len() > MAX_PATH_BYTES {
        return Err(FunctionCallError::RespondToModel(
            "file path must be non-empty and at most 4096 UTF-8 bytes".to_string(),
        ));
    }
    let environment = resolve_tool_environment(&step_context.environments, environment_id)?
        .ok_or_else(|| {
            FunctionCallError::RespondToModel("no execution environment is available".to_string())
        })?;
    if !multi_environment && environment_id.is_some() {
        return Err(FunctionCallError::RespondToModel(
            "environment_id is only valid when multiple environments are selected".to_string(),
        ));
    }
    let sandbox = turn.file_system_sandbox_context(None, environment);
    let joined = environment
        .cwd()
        .join(raw_path)
        .map_err(|err| FunctionCallError::RespondToModel(format!("invalid file path: {err}")))?;
    let fs = environment.environment.get_filesystem();
    let canonical_path = match fs.canonicalize(&joined, Some(&sandbox)).await {
        Ok(path) => path,
        Err(_) => {
            let Some(parent) = joined.parent() else {
                return Err(FunctionCallError::RespondToModel(
                    "file path has no canonicalizable parent".to_string(),
                ));
            };
            let parent = fs
                .canonicalize(&parent, Some(&sandbox))
                .await
                .map_err(file_io_error)?;
            let basename = joined.basename().ok_or_else(|| {
                FunctionCallError::RespondToModel("file path must name a regular file".to_string())
            })?;
            parent.join(&basename).map_err(|err| {
                FunctionCallError::RespondToModel(format!("invalid file path: {err}"))
            })?
        }
    };
    let mut inside_workspace = false;
    for root in environment.workspace_roots() {
        if let Ok(root) = fs.canonicalize(root, Some(&sandbox)).await
            && canonical_path.starts_with(&root)
        {
            inside_workspace = true;
            break;
        }
    }
    if !inside_workspace {
        return Err(FunctionCallError::RespondToModel(
            "file path must stay inside an allowed workspace root".to_string(),
        ));
    }
    Ok((environment, joined, canonical_path, sandbox))
}

pub(super) fn file_io_error(error: std::io::Error) -> FunctionCallError {
    FunctionCallError::RespondToModel(format!("file operation failed: {error}"))
}
