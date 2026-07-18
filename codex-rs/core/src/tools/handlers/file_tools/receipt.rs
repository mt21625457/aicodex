use super::*;
use crate::function_tool::FunctionCallError;
use codex_file_system::FileMetadata;

pub(super) async fn validate_receipt(
    state: &Arc<Mutex<FileToolState>>,
    key: &FileReceiptKey,
    bytes: &[u8],
    metadata: &FileMetadata,
    step_context: &Arc<StepContext>,
    require_full: bool,
) -> Result<(), FunctionCallError> {
    let mut state = state.lock().await;
    state.next_access = state.next_access.saturating_add(1);
    let access = state.next_access;
    let Some(receipt) = state.receipts.get_mut(key) else {
        return Err(FunctionCallError::RespondToModel(
            "read_file is required before editing or overwriting this file".to_string(),
        ));
    };
    if receipt.step_id == step_id(step_context) {
        return Err(FunctionCallError::RespondToModel(
            "read_file and dependent mutation must be issued in separate completions".to_string(),
        ));
    }
    if require_full && (!receipt.full_coverage || !receipt.write_eligible) {
        return Err(FunctionCallError::RespondToModel(
            "a complete read is required before overwriting this file".to_string(),
        ));
    }
    let metadata_changed = receipt.modified_at_ms != metadata.modified_at_ms;
    if receipt.file_size != metadata.size || receipt.fingerprint != fingerprint(bytes) {
        return Err(FunctionCallError::RespondToModel(
            if metadata_changed {
                "file content and metadata changed since read_file; read it again before editing"
            } else {
                "file changed since read_file; read it again before editing"
            }
            .to_string(),
        ));
    }
    receipt.modified_at_ms = metadata.modified_at_ms;
    receipt.last_used = access;
    Ok(())
}

fn receipt_accounted_bytes(key: &FileReceiptKey, receipt: &FileReceipt) -> usize {
    key.environment_id
        .len()
        .saturating_add(key.path.to_string().len())
        .saturating_add(std::mem::size_of::<FileReceipt>())
        .saturating_add(
            receipt
                .observed_ranges
                .len()
                .saturating_mul(std::mem::size_of::<(usize, usize)>()),
        )
}

pub(super) fn remove_receipt(state: &mut FileToolState, key: &FileReceiptKey) {
    if let Some(receipt) = state.receipts.remove(key) {
        state.total_ranges = state
            .total_ranges
            .saturating_sub(receipt.observed_ranges.len());
        state.total_bytes = state.total_bytes.saturating_sub(receipt.accounted_bytes);
    }
}

pub(super) fn insert_receipt(
    state: &mut FileToolState,
    key: FileReceiptKey,
    mut receipt: FileReceipt,
) -> Result<(), FunctionCallError> {
    if receipt.observed_ranges.len() > MAX_RECEIPT_RANGES_PER_ENTRY {
        remove_receipt(state, &key);
        return Err(FunctionCallError::RespondToModel(
            "too many observed ranges; read a larger contiguous range".to_string(),
        ));
    }
    receipt.accounted_bytes = receipt_accounted_bytes(&key, &receipt);
    if receipt.accounted_bytes > MAX_RECEIPT_BYTES {
        return Err(FunctionCallError::RespondToModel(
            "file read receipt exceeds the bounded receipt store".to_string(),
        ));
    }
    remove_receipt(state, &key);
    while (state.receipts.len() >= MAX_RECEIPTS
        || state
            .total_ranges
            .saturating_add(receipt.observed_ranges.len())
            > MAX_RECEIPT_RANGES
        || state.total_bytes.saturating_add(receipt.accounted_bytes) > MAX_RECEIPT_BYTES)
        && !state.receipts.is_empty()
    {
        let Some(evicted) = state
            .receipts
            .iter()
            .min_by_key(|(_, receipt)| receipt.last_used)
            .map(|(key, _)| key.clone())
        else {
            break;
        };
        remove_receipt(state, &evicted);
    }
    if state.receipts.len() >= MAX_RECEIPTS
        || state
            .total_ranges
            .saturating_add(receipt.observed_ranges.len())
            > MAX_RECEIPT_RANGES
        || state.total_bytes.saturating_add(receipt.accounted_bytes) > MAX_RECEIPT_BYTES
    {
        return Err(FunctionCallError::RespondToModel(
            "file read receipt store is full; read a larger contiguous range or retry".to_string(),
        ));
    }
    state.next_access = state.next_access.saturating_add(1);
    receipt.last_used = state.next_access;
    state.total_ranges = state
        .total_ranges
        .saturating_add(receipt.observed_ranges.len());
    state.total_bytes = state.total_bytes.saturating_add(receipt.accounted_bytes);
    state.receipts.insert(key, receipt);
    Ok(())
}

pub(super) async fn validate_observed_ranges(
    state: &Arc<Mutex<FileToolState>>,
    key: &FileReceiptKey,
    text: &str,
    old_string: &str,
) -> Result<(), FunctionCallError> {
    let mut state = state.lock().await;
    state.next_access = state.next_access.saturating_add(1);
    let access = state.next_access;
    let Some(receipt) = state.receipts.get_mut(key) else {
        return Err(FunctionCallError::RespondToModel(
            "read_file is required before editing this file".to_string(),
        ));
    };
    // Complete reads already cover every lines()-numbered line; skip span math that
    // disagrees with trailing newlines (`"a\n".lines()` is one line, but `\n` count is 1).
    if !receipt.full_coverage {
        for start in text.match_indices(old_string).map(|(start, _)| start) {
            let line = text[..start].bytes().filter(|byte| *byte == b'\n').count() + 1;
            let newline_count = old_string.bytes().filter(|byte| *byte == b'\n').count();
            let match_end = start.saturating_add(old_string.len());
            let end = if newline_count == 0 {
                line
            } else if old_string.ends_with('\n') && match_end == text.len() {
                // Trailing newline at EOF does not create an extra lines() row.
                line + newline_count - 1
            } else {
                line + newline_count
            };
            if !receipt
                .observed_ranges
                .iter()
                .any(|(range_start, range_end)| line >= *range_start && end <= *range_end)
            {
                return Err(FunctionCallError::RespondToModel(
                    "the replacement is outside the observed read range; read that range again"
                        .to_string(),
                ));
            }
        }
    }
    receipt.last_used = access;
    Ok(())
}

pub(super) async fn refresh_receipt_after_commit(
    state: &Arc<Mutex<FileToolState>>,
    key: &FileReceiptKey,
    fs: &dyn codex_file_system::ExecutorFileSystem,
    path: &PathUri,
    sandbox: &codex_file_system::FileSystemSandboxContext,
    step_context: &Arc<StepContext>,
) {
    let Ok(bytes) = fs.read_file(path, Some(sandbox)).await else {
        let mut guard = state.lock().await;
        remove_receipt(&mut guard, key);
        return;
    };
    let Ok(metadata) = fs.get_metadata(path, Some(sandbox)).await else {
        let mut guard = state.lock().await;
        remove_receipt(&mut guard, key);
        return;
    };
    let receipt = FileReceipt {
        fingerprint: fingerprint(&bytes),
        file_size: metadata.size,
        modified_at_ms: metadata.modified_at_ms,
        full_coverage: true,
        write_eligible: true,
        observed_ranges: vec![(1, usize::MAX)],
        step_id: step_id(step_context),
        last_used: 0,
        accounted_bytes: 0,
    };
    let mut guard = state.lock().await;
    let _ = insert_receipt(&mut guard, key.clone(), receipt);
}

pub(super) fn step_id(step_context: &Arc<StepContext>) -> u64 {
    step_context.id()
}

pub(super) fn fingerprint(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}
