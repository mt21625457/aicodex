//! Responses 原始推理回放共用的完整项压缩策略。
use super::*;

/// 压缩保留目标而非逐项硬截断；最新完整项超过目标时独占保留区。
/// 普通请求仍由模型上下文预算负责，不能在工具回合中间截断推理。
const COMPACT_REASONING_TARGET_TOKENS: usize = 64_000;

/// 从最后真实用户输入之后选取连续的最新完整推理项，不修改持久化原文。
/// 64K 目标不足时丢弃更早项；最新单项超限仍完整保留并记录结构化诊断。
pub(super) fn last_turn_complete_reasoning(
    items: &[ResponseItemEnvelope],
    model_slug: &str,
) -> Vec<ResponseItemEnvelope> {
    let summary_prefix = format!("{SUMMARY_PREFIX}\n");
    let Some(last_user) = items.iter().rposition(|envelope| {
        matches!(parse_turn_item(&envelope.item), Some(TurnItem::UserMessage(user))
            if !user.message().starts_with(&summary_prefix))
    }) else {
        return Vec::new();
    };
    let mut selected = Vec::new();
    let mut retained_tokens = 0usize;
    for envelope in items[last_user + 1..].iter().rev() {
        let ResponseItem::Reasoning { content, .. } = &envelope.item else {
            continue;
        };
        let tokens = raw_reasoning_content_tokens(content);
        if tokens == 0 {
            continue;
        }
        if !selected.is_empty()
            && retained_tokens.saturating_add(tokens) > COMPACT_REASONING_TARGET_TOKENS
        {
            tracing::debug!(
                operation = "compact_reasoning",
                model = model_slug,
                retained_tokens,
                target_tokens = COMPACT_REASONING_TARGET_TOKENS,
                "Stopped at complete reasoning item boundary"
            );
            break;
        }
        let mut retained = envelope.clone();
        sanitize_raw_reasoning_for_replay(std::slice::from_mut(&mut retained.item));
        selected.push(retained);
        retained_tokens = retained_tokens.saturating_add(tokens);
        if retained_tokens >= COMPACT_REASONING_TARGET_TOKENS {
            if retained_tokens > COMPACT_REASONING_TARGET_TOKENS {
                tracing::warn!(
                    code = "complete_reasoning_over_target",
                    operation = "compact",
                    model = model_slug,
                    retained_tokens,
                    target_tokens = COMPACT_REASONING_TARGET_TOKENS,
                    "Kept newest reasoning item intact above compaction target"
                );
            }
            break;
        }
    }
    selected.reverse();
    selected
}

#[cfg(test)]
#[path = "responses_complete_reasoning_tests.rs"]
mod tests;
