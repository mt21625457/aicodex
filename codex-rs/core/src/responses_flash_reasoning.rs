//! Flash Responses 的完整推理保留策略；不能扩大到 Pro、MiniMax 或未知未来模型。
use super::*;

/// 压缩保留目标而非逐项硬截断；最新完整项超过目标时独占保留区。
/// 普通请求仍由模型上下文预算负责，不能在工具回合中间截断推理。
const COMPACT_REASONING_TARGET_TOKENS: usize = 64_000;

/// 仅接受精确 Flash slug 及网关前缀，不能用 deepseek-* 家族匹配。
pub(super) fn is_flash(model: &str) -> bool {
    let model = model.trim();
    // 与 DeepSeek 家族识别保持一致：路径前缀后的 :free 等路由后缀
    // 不属于模型名；无路径时，冒号用于分隔网关前缀与模型名。
    let slug = match model.rsplit_once('/') {
        Some((_, slug)) => slug.split(':').next().unwrap_or(slug),
        None => model.rsplit(':').next().unwrap_or(model),
    };
    slug.trim().eq_ignore_ascii_case("deepseek-flash")
}

/// 从最后真实用户输入之后选取连续的最新完整推理项，不修改持久化原文。
/// 64K 目标不足时丢弃更早项；最新单项超限仍完整保留并记录结构化诊断。
pub(super) fn last_turn_complete_reasoning(
    items: &[ResponseItemEnvelope],
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
                operation = "flash_compact_reasoning",
                model = "deepseek-flash",
                retained_tokens,
                target_tokens = COMPACT_REASONING_TARGET_TOKENS,
                "Stopped at complete reasoning item boundary"
            );
            break;
        }
        let mut retained = envelope.clone();
        sanitize_raw_reasoning_for_replay(std::slice::from_mut(&mut retained.item), usize::MAX);
        selected.push(retained);
        retained_tokens = retained_tokens.saturating_add(tokens);
        if retained_tokens >= COMPACT_REASONING_TARGET_TOKENS {
            if retained_tokens > COMPACT_REASONING_TARGET_TOKENS {
                tracing::warn!(
                    code = "flash_complete_reasoning_over_target",
                    operation = "compact",
                    model = "deepseek-flash",
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
#[path = "responses_flash_reasoning_tests.rs"]
mod tests;
