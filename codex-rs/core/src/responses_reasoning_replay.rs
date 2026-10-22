//! Responses-wire reasoning replay policy.
//!
//! Official OpenAI Responses continues thinking with `encrypted_content` and
//! rejects raw `reasoning_text` as input. DeepSeek and MiniMax (including
//! gateway-prefixed slugs) need that plaintext replayed or the next turn is
//! downgraded to non-thinking and can stall or repeat after tool results.

use crate::event_mapping::parse_turn_item;
use codex_history::ResponseItemEnvelope;
use codex_models_manager::model_info::is_deepseek_model_slug;
use codex_models_manager::model_info::is_minimax_model_slug;
use codex_prompts::SUMMARY_PREFIX;
use codex_protocol::items::TurnItem;
use codex_protocol::models::ReasoningItemContent;
use codex_protocol::models::ResponseItem;
use codex_utils_output_truncation::approx_token_count;

#[path = "responses_complete_reasoning.rs"]
mod complete;

/// How historical reasoning is replayed on the Responses wire.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ResponsesReasoningReplay {
    /// OpenAI / OpenAI-compatible: omit raw content; keep encrypted handoff
    /// only for trusted OpenAI-origin items.
    EncryptedHandoff { is_openai_provider: bool },
    /// DeepSeek / MiniMax: keep `reasoning_text` so the next turn can continue thinking.
    RawReasoningText,
}

pub(crate) fn responses_reasoning_replay(
    model_slug: &str,
    is_openai_provider: bool,
) -> ResponsesReasoningReplay {
    if is_deepseek_model_slug(model_slug) || is_minimax_model_slug(model_slug) {
        ResponsesReasoningReplay::RawReasoningText
    } else {
        ResponsesReasoningReplay::EncryptedHandoff { is_openai_provider }
    }
}

pub(crate) fn strip_reasoning_content_for_responses_input(
    input: &mut [ResponseItem],
    replay: ResponsesReasoningReplay,
) {
    match replay {
        ResponsesReasoningReplay::RawReasoningText => {
            // 所有原始推理回放均保持完整；上下文压力交由压缩按完整项处理。
            sanitize_raw_reasoning_for_replay(input);
        }
        ResponsesReasoningReplay::EncryptedHandoff { is_openai_provider } => {
            strip_encrypted_handoff_reasoning(input, is_openai_provider);
        }
    }
}

fn sanitize_raw_reasoning_for_replay(input: &mut [ResponseItem]) {
    for item in input {
        let ResponseItem::Reasoning {
            id,
            summary,
            content,
            encrypted_content,
            ..
        } = item
        else {
            continue;
        };

        // Raw reasoning text is portable across DeepSeek-compatible providers,
        // but IDs, summaries, encrypted handoffs, and signatures are not.
        *id = None;
        summary.clear();
        *encrypted_content = None;
        let tokens = normalize_raw_reasoning_content(content);
        if tokens == 0 {
            *content = Some(Vec::new());
            continue;
        }
    }
}

fn normalize_raw_reasoning_content(content: &mut Option<Vec<ReasoningItemContent>>) -> usize {
    for part in content.iter_mut().flatten() {
        if let ReasoningItemContent::Text { text } = part {
            *part = ReasoningItemContent::ReasoningText {
                text: std::mem::take(text),
            };
        }
    }
    raw_reasoning_content_tokens(content)
}

fn raw_reasoning_content_tokens(content: &Option<Vec<ReasoningItemContent>>) -> usize {
    content
        .iter()
        .flatten()
        .map(|part| match part {
            ReasoningItemContent::ReasoningText { text } | ReasoningItemContent::Text { text } => {
                approx_token_count(text)
            }
        })
        .sum()
}

fn strip_encrypted_handoff_reasoning(input: &mut [ResponseItem], is_openai_provider: bool) {
    for item in input {
        if let ResponseItem::Reasoning {
            id,
            content,
            encrypted_content,
            ..
        } = item
        {
            let has_raw_reasoning_content =
                content.as_ref().is_some_and(|content| !content.is_empty());
            // Responses reasoning items are replayed with encrypted_content/summary.
            // Raw reasoning_text content is output-only; sending it back as input
            // causes OpenAI-compatible Responses providers to reject the request.
            // Claude-wire reasoning signatures are a different provider-specific
            // state format and must not be replayed as OpenAI encrypted reasoning.
            // Older persisted histories may not have retained the reasoning item
            // id, so unknown-origin raw reasoning also cannot safely carry
            // encrypted_content into a Responses request.
            //
            // Use an empty vector because the protocol serializer currently omits
            // empty reasoning content, while None serializes as content: null.
            *content = Some(Vec::new());
            let id = id.as_deref().unwrap_or_default();
            let unknown_origin_raw_reasoning = id.is_empty() && has_raw_reasoning_content;
            let non_openai_raw_reasoning = !is_openai_provider && has_raw_reasoning_content;
            if id
                .rsplit_once("_reasoning_")
                .is_some_and(|(_, index)| index.parse::<usize>().is_ok())
                || unknown_origin_raw_reasoning
                || non_openai_raw_reasoning
            {
                *encrypted_content = None;
            }
        }
    }
}

/// Last-turn `Reasoning` items to keep in compacted replacement history.
///
/// Only DeepSeek / MiniMax raw replay keeps these items. Earlier turns are
/// dropped, and the newest items are preferred when the last turn exceeds the
/// 64K soft target. Retained items are never truncated, even if the newest
/// item alone exceeds the target.
pub(crate) fn last_turn_reasoning_for_raw_replay(
    items: &[ResponseItemEnvelope],
    model_slug: &str,
    is_openai_provider: bool,
) -> Vec<ResponseItemEnvelope> {
    if !matches!(
        responses_reasoning_replay(model_slug, is_openai_provider),
        ResponsesReasoningReplay::RawReasoningText
    ) {
        return Vec::new();
    }

    complete::last_turn_complete_reasoning(items, model_slug)
}

#[cfg(test)]
#[path = "responses_reasoning_replay_tests.rs"]
mod tests;
