use super::io::file_io_error;
use super::io::read_editable_file;
use super::io::read_file_prefix;
use super::io::resolve_file_target;
use super::mutation::ReviewableFileMutation;
use super::mutation::commit_reviewable_mutation;
use super::receipt::fingerprint;
use super::receipt::insert_receipt;
use super::receipt::refresh_receipt_after_commit;
use super::receipt::remove_receipt;
use super::receipt::step_id;
use super::receipt::validate_observed_ranges;
use super::receipt::validate_receipt;
use super::schema::EditFileArgs;
use super::schema::ReadFileArgs;
use super::schema::WriteFileArgs;
use super::text::decode_scanned_text;
use super::text::decode_text;
use super::text::encode_text;
use super::*;
use crate::function_tool::FunctionCallError;
use serde::Deserialize;

#[cfg(test)]
use super::schema::file_schema;

pub(super) async fn read_file(
    state: Arc<Mutex<FileToolState>>,
    multi_environment: bool,
    invocation: ToolInvocation,
) -> Result<Box<dyn ToolOutput>, FunctionCallError> {
    let args: ReadFileArgs = function_args(&invocation.payload)?;
    if args.path.len() > MAX_PATH_BYTES
        || args.offset == 0
        || args.offset > 1_000_000
        || args.limit == 0
        || args.limit > MAX_LINES
    {
        return Err(FunctionCallError::RespondToModel(
            "read_file requires a path up to 4096 bytes, offset 1..=1000000, and limit 1..=2000"
                .to_string(),
        ));
    }
    let (environment, path, canonical_path, sandbox) = resolve_file_target(
        &invocation.step_context,
        &invocation.turn,
        args.environment_id.as_deref(),
        &args.path,
        multi_environment,
    )
    .await?;
    let fs = environment.environment.get_filesystem();
    let metadata = fs
        .get_metadata(&path, Some(&sandbox))
        .await
        .map_err(file_io_error)?;
    if metadata.is_directory || !metadata.is_file {
        return Err(FunctionCallError::RespondToModel(
            "read_file supports regular text files only".to_string(),
        ));
    }
    let (bytes, reached_eof) =
        read_file_prefix(fs.as_ref(), &path, &sandbox, MAX_SCAN_BYTES).await?;
    let file_within_editable_cap = reached_eof && bytes.len() <= MAX_FILE_BYTES;
    let scanned_fingerprint = fingerprint(&bytes);
    if !reached_eof && !bytes.contains(&b'\n') {
        return Err(FunctionCallError::RespondToModel(
            "read_file reached the 64 MiB scan limit before a complete requested line; use a specialized script for this file"
                .to_string(),
        ));
    }
    let text = decode_scanned_text(bytes, reached_eof).map_err(|_| {
        FunctionCallError::RespondToModel(
            "read_file supports UTF-8 and round-trippable legacy text; use a specialized tool for binary or unsupported encoding"
                .to_string(),
        )
    })?
    .text;
    let lines = text.lines().collect::<Vec<_>>();
    if !reached_eof && args.offset > lines.len() {
        return Err(FunctionCallError::RespondToModel(
            "read_file reached the 64 MiB scan limit before the requested range; use a specialized script for this file"
                .to_string(),
        ));
    }
    if reached_eof && args.offset > lines.len().saturating_add(1) {
        return Err(FunctionCallError::RespondToModel(
            "read_file offset exceeds the file length".to_string(),
        ));
    }
    let start = args.offset.saturating_sub(1);
    let end = start.saturating_add(args.limit).min(lines.len());
    let content = lines[start..end]
        .iter()
        .enumerate()
        .map(|(index, line)| format!("L{}: {line}", start + index + 1))
        .collect::<Vec<_>>();
    let complete = reached_eof && end == lines.len() && start == 0;
    let write_eligible = file_within_editable_cap && complete;
    let header = |complete| {
        format!(
            "Path: {}\nLines: {}-{}\nTotal lines: {}\nComplete: {}\nWrite eligible: {}\nReceipt scope: current turn\nContent:\n{}",
            path,
            if lines.is_empty() { 1 } else { start + 1 },
            if lines.is_empty() { 0 } else { end },
            if reached_eof {
                lines.len().to_string()
            } else {
                "unknown".to_string()
            },
            complete,
            write_eligible,
            "",
        )
    };
    let mut output = header(complete);
    let mut output_truncated = false;
    let mut rendered_end = start;
    for line in content {
        let addition = if output.ends_with("\n") {
            format!("{line}\n")
        } else {
            format!("\n{line}")
        };
        let output_cap = MAX_OUTPUT_BYTES.min(MAX_OUTPUT_TOKEN_BYTES);
        if output
            .len()
            .saturating_add(addition.len())
            .saturating_add(OUTPUT_TRUNCATION_MARKER.len())
            > output_cap
        {
            output_truncated = true;
            break;
        }
        output.push_str(&addition);
        rendered_end += 1;
    }
    if output_truncated {
        output = output.replacen("Complete: true", "Complete: false", 1);
        output = output.replacen("Write eligible: true", "Write eligible: false", 1);
        output.push_str(OUTPUT_TRUNCATION_MARKER);
    }
    let key = FileReceiptKey {
        environment_id: environment.environment_id.clone(),
        path: canonical_path,
    };
    let mut receipt = FileReceipt {
        fingerprint: scanned_fingerprint,
        file_size: metadata.size,
        modified_at_ms: metadata.modified_at_ms,
        full_coverage: reached_eof
            && start == 0
            && rendered_end == lines.len()
            && file_within_editable_cap,
        write_eligible: file_within_editable_cap,
        observed_ranges: if rendered_end > start {
            vec![(start + 1, rendered_end)]
        } else {
            Vec::new()
        },
        step_id: step_id(&invocation.step_context),
        last_used: 0,
        accounted_bytes: 0,
    };
    let mut state = state.lock().await;
    if let Some(previous) = state.receipts.get(&key).cloned()
        && previous.fingerprint == receipt.fingerprint
    {
        let mut ranges = previous.observed_ranges.clone();
        ranges.extend(receipt.observed_ranges.iter().copied());
        ranges.sort_unstable();
        let mut merged: Vec<(usize, usize)> = Vec::with_capacity(ranges.len());
        for (start, end) in ranges {
            if let Some((_, previous_end)) = merged.last_mut()
                && start <= previous_end.saturating_add(1)
            {
                *previous_end = (*previous_end).max(end);
            } else {
                merged.push((start, end));
            }
        }
        if merged.len() > MAX_RECEIPT_RANGES_PER_ENTRY {
            remove_receipt(&mut state, &key);
            return Err(FunctionCallError::RespondToModel(
                "too many observed ranges; read a larger contiguous range".to_string(),
            ));
        }
        receipt.observed_ranges = merged;
        receipt.full_coverage |= previous.full_coverage;
    }
    insert_receipt(&mut state, key, receipt)?;
    Ok(boxed_tool_output(FunctionToolOutput::from_text(
        output,
        Some(true),
    )))
}

pub(super) async fn edit_file(
    state: Arc<Mutex<FileToolState>>,
    multi_environment: bool,
    invocation: ToolInvocation,
) -> Result<Box<dyn ToolOutput>, FunctionCallError> {
    let args: EditFileArgs = mutation_args(&invocation.payload, "edit_file")?;
    if args.path.len() > MAX_PATH_BYTES {
        return Err(FunctionCallError::RespondToModel(
            "edit_file path exceeds the 4096-byte limit".to_string(),
        ));
    }
    if args.old_string.is_empty() {
        return Err(FunctionCallError::RespondToModel(
            "edit_file old_string must be non-empty".to_string(),
        ));
    }
    let (environment, path, canonical_path, sandbox) = resolve_file_target(
        &invocation.step_context,
        &invocation.turn,
        args.environment_id.as_deref(),
        &args.path,
        multi_environment,
    )
    .await?;
    let fs = environment.environment.get_filesystem();
    let (bytes, metadata) = read_editable_file(fs.as_ref(), &path, &sandbox, "edit_file").await?;
    let original_fingerprint = fingerprint(&bytes);
    let key = FileReceiptKey {
        environment_id: environment.environment_id.clone(),
        path: canonical_path,
    };
    validate_receipt(
        &state,
        &key,
        &bytes,
        &metadata,
        &invocation.step_context,
        false,
    )
    .await?;
    let decoded = decode_text(bytes).map_err(|_| {
        FunctionCallError::RespondToModel(
            "edit_file supports UTF-8 and round-trippable legacy text only".to_string(),
        )
    })?;
    let text = decoded.text;
    let matches = text.matches(&args.old_string).count();
    if matches == 0 || (matches > 1 && !args.replace_all) {
        return Err(FunctionCallError::RespondToModel(format!(
            "edit_file expected one match (or replace_all=true), found {matches}"
        )));
    }
    validate_observed_ranges(&state, &key, &text, &args.old_string).await?;
    let updated_len = text
        .len()
        .checked_sub(matches.saturating_mul(args.old_string.len()))
        .and_then(|len| len.checked_add(matches.saturating_mul(args.new_string.len())))
        .ok_or_else(|| {
            FunctionCallError::RespondToModel(
                "edit_file replacement exceeds the 8 MiB editable limit; use a specialized script"
                    .to_string(),
            )
        })?;
    if updated_len > MAX_FILE_BYTES {
        return Err(FunctionCallError::RespondToModel(
            "edit_file replacement exceeds the 8 MiB editable limit; use a specialized script"
                .to_string(),
        ));
    }
    let updated = if args.replace_all {
        text.replace(&args.old_string, &args.new_string)
    } else {
        text.replacen(&args.old_string, &args.new_string, 1)
    };
    codex_apply_patch::decode_patchable_text(updated.as_bytes().to_vec()).map_err(|_| {
        FunctionCallError::RespondToModel(
            "edit_file replacement would create binary or unsupported text".to_string(),
        )
    })?;
    let updated_for_event = updated.clone();
    let updated_bytes = encode_text(
        &updated,
        decoded.encoding,
        decoded.had_crlf,
        decoded.utf8_bom,
    )
    .map_err(file_io_error)?;
    if updated_bytes.len() > MAX_FILE_BYTES {
        return Err(FunctionCallError::RespondToModel(
            "edit_file encoded result exceeds the 8 MiB editable limit; use a specialized script"
                .to_string(),
        ));
    }
    commit_reviewable_mutation(
        &invocation,
        environment,
        ReviewableFileMutation {
            path: &path,
            canonical_path: &key.path,
            contents: updated_bytes,
            precondition: ConditionalWritePrecondition::MatchSha256(original_fingerprint),
            old_text: Some(&text),
            new_text: &updated_for_event,
        },
    )
    .await?;
    refresh_receipt_after_commit(
        &state,
        &key,
        fs.as_ref(),
        &path,
        &sandbox,
        &invocation.step_context,
    )
    .await;
    Ok(boxed_tool_output(FunctionToolOutput::from_text(
        "edit_file completed".to_string(),
        Some(true),
    )))
}

pub(super) async fn write_file(
    state: Arc<Mutex<FileToolState>>,
    multi_environment: bool,
    invocation: ToolInvocation,
) -> Result<Box<dyn ToolOutput>, FunctionCallError> {
    let args: WriteFileArgs = mutation_args(&invocation.payload, "write_file")?;
    if args.path.len() > MAX_PATH_BYTES || args.content.len() > MAX_FILE_BYTES {
        return Err(FunctionCallError::RespondToModel(
            "write_file path exceeds 4096 bytes or content exceeds the 8 MiB editable limit"
                .to_string(),
        ));
    }
    codex_apply_patch::decode_patchable_text(args.content.as_bytes().to_vec()).map_err(|_| {
        FunctionCallError::RespondToModel(
            "write_file content contains binary or unsupported text".to_string(),
        )
    })?;
    let (environment, path, canonical_path, sandbox) = resolve_file_target(
        &invocation.step_context,
        &invocation.turn,
        args.environment_id.as_deref(),
        &args.path,
        multi_environment,
    )
    .await?;
    let fs = environment.environment.get_filesystem();
    let key = FileReceiptKey {
        environment_id: environment.environment_id.clone(),
        path: canonical_path,
    };
    let existing = match fs.get_metadata(&path, Some(&sandbox)).await {
        Ok(_) => Some(read_editable_file(fs.as_ref(), &path, &sandbox, "write_file").await?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(file_io_error(error)),
    };
    let existing_text = existing
        .as_ref()
        .map(|(bytes, _)| decode_text(bytes.clone()))
        .transpose()
        .map_err(file_io_error)?;
    let precondition = if let Some((bytes, metadata)) = existing.as_ref() {
        validate_receipt(
            &state,
            &key,
            bytes,
            metadata,
            &invocation.step_context,
            true,
        )
        .await?;
        ConditionalWritePrecondition::MatchSha256(fingerprint(bytes))
    } else {
        ConditionalWritePrecondition::MustNotExist
    };
    let content_for_event = args.content.clone();
    let content = match existing_text.as_ref() {
        Some(existing_text) => encode_text(
            &args.content,
            existing_text.encoding,
            existing_text.had_crlf,
            existing_text.utf8_bom,
        )
        .map_err(file_io_error)?,
        None => args.content.into_bytes(),
    };
    let old_text = existing_text.as_ref().map(|decoded| decoded.text.as_str());
    commit_reviewable_mutation(
        &invocation,
        environment,
        ReviewableFileMutation {
            path: &path,
            canonical_path: &key.path,
            contents: content,
            precondition,
            old_text,
            new_text: &content_for_event,
        },
    )
    .await?;
    refresh_receipt_after_commit(
        &state,
        &key,
        fs.as_ref(),
        &path,
        &sandbox,
        &invocation.step_context,
    )
    .await;
    let message = if existing.is_some() {
        "write_file overwrote the file"
    } else {
        "write_file created the file"
    };
    Ok(boxed_tool_output(FunctionToolOutput::from_text(
        message.to_string(),
        Some(true),
    )))
}

fn function_args<T: for<'de> Deserialize<'de>>(
    payload: &ToolPayload,
) -> Result<T, FunctionCallError> {
    let ToolPayload::Function { arguments } = payload else {
        return Err(FunctionCallError::RespondToModel(
            "dedicated file tool received an unsupported payload".to_string(),
        ));
    };
    parse_arguments(arguments)
}

fn mutation_args<T: for<'de> Deserialize<'de>>(
    payload: &ToolPayload,
    tool_name: &str,
) -> Result<T, FunctionCallError> {
    let ToolPayload::Function { arguments } = payload else {
        return Err(FunctionCallError::RespondToModel(format!(
            "{tool_name} received an unsupported payload"
        )));
    };
    let cap = MAX_MUTATION_ARGUMENT_BYTES.min(MAX_MUTATION_TOKEN_BYTES);
    if arguments.len() > cap {
        return Err(FunctionCallError::RespondToModel(format!(
            "{tool_name} arguments exceed the 64 KiB / approximately 10,000-token limit; use a smaller edit or a specialized script"
        )));
    }
    parse_arguments(arguments)
}

#[cfg(test)]
#[path = "../file_tools_tests.rs"]
mod tests;
