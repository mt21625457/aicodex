use codex_protocol::models::ResponseItem;

const MAX_ITEM_ID_CHARS: usize = 64;

/// Operates on the wire copy only: saved history and tool correlation IDs stay intact.
/// Opaque/stateful identities cannot be shortened or silently discarded.
pub(crate) fn sanitize_overlong_item_ids(input: &mut [ResponseItem]) -> Result<(), String> {
    let mut needs_cleanup = false;
    for (index, item) in input.iter().enumerate() {
        let Some(length) = overlong_id_length(item) else {
            continue;
        };
        if !can_omit_item_id(item) {
            return Err(format!(
                "Invalid input[{index}].id ({}): length {length} exceeds {MAX_ITEM_ID_CHARS}; \
                 this stateful item cannot safely be replayed without its original ID",
                item.id_prefix().unwrap_or("unknown")
            ));
        }
        needs_cleanup = true;
    }
    if !needs_cleanup {
        return Ok(());
    }
    for item in input {
        if overlong_id_length(item).is_some() {
            item.set_id(None);
        }
    }
    Ok(())
}

fn overlong_id_length(item: &ResponseItem) -> Option<usize> {
    let id = item.id()?;
    // ASCII IDs take the fast path. Match a string-length limit for legacy Unicode IDs.
    if id.len() <= MAX_ITEM_ID_CHARS {
        return None;
    }
    let length = if id.is_ascii() {
        id.len()
    } else {
        id.chars().count()
    };
    (length > MAX_ITEM_ID_CHARS).then_some(length)
}

fn can_omit_item_id(item: &ResponseItem) -> bool {
    match item {
        ResponseItem::Message { content, .. } => !content.is_empty(),
        ResponseItem::FunctionCall {
            call_id,
            encrypted_function_args,
            ..
        } => !call_id.is_empty() && encrypted_function_args.is_none(),
        ResponseItem::CustomToolCall { call_id, .. }
        | ResponseItem::CustomToolCallOutput { call_id, .. } => !call_id.is_empty(),
        ResponseItem::FunctionCallOutput { call_id, .. } => {
            call_id.as_ref().is_some_and(|id| !id.is_empty())
        }
        _ => false,
    }
}

#[cfg(test)]
#[path = "responses_item_id_tests.rs"]
mod tests;
