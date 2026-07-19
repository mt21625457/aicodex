use super::session::Session;
use super::turn_context::TurnContext;
use crate::compact::should_use_remote_compact_task;
use codex_features::Feature;
use codex_model_provider_info::WireApi;
use codex_protocol::config_types::AutoCompactTokenLimitScope;
use codex_protocol::protocol::ContextTokenUsageSource;

#[derive(Debug)]
pub(crate) struct ContextWindowTokenStatus {
    // Full active context usage, independent of the configured auto-compact scope.
    pub(crate) active_context_tokens: i64,
    // Usage counted against `model_auto_compact_token_limit` for the current scope.
    pub(crate) auto_compact_scope_tokens: i64,
    pub(crate) auto_compact_scope_limit: Option<i64>,
    pub(crate) full_context_window_limit: Option<i64>,
    pub(crate) base_window_tokens_remaining: Option<i64>,
    pub(crate) auto_compact_window_prefill_tokens: Option<i64>,
    pub(crate) full_context_window_limit_reached: bool,
    pub(crate) token_limit_reached: bool,
}

fn tokens_remaining(limit: Option<i64>, used: i64) -> Option<i64> {
    limit.map(|limit| limit.saturating_sub(used).max(0))
}

pub(crate) async fn context_window_token_status(
    sess: &Session,
    turn_context: &TurnContext,
) -> ContextWindowTokenStatus {
    let active_context_tokens =
        recorded_context_tokens_for_context_window(sess, turn_context).await;

    let auto_compact_context_limit = turn_context
        .model_info
        .resolved_context_window()
        .map(|context_window| (context_window * 9) / 10);
    let clamp_to_auto_compact_context = |limit: i64| {
        auto_compact_context_limit
            .map(|context_limit| limit.min(context_limit))
            .unwrap_or(limit)
    };
    let explicit_auto_compact_token_limit =
        turn_context
            .config
            .model_auto_compact_token_limit
            .map(|limit| {
                if should_use_remote_compact_task(turn_context.provider.info()) {
                    limit
                } else {
                    clamp_to_auto_compact_context(limit)
                }
            });
    let model_auto_compact_token_limit = if turn_context.config.model_context_window.is_some() {
        turn_context.model_info.auto_compact_token_limit
    } else {
        turn_context.model_info.auto_compact_token_limit()
    };
    let token_budget_context_limit = turn_context
        .config
        .features
        .enabled(Feature::TokenBudget)
        .then_some(auto_compact_context_limit)
        .flatten();

    let (auto_compact_scope_tokens, auto_compact_scope_limit, auto_compact_window_prefill_tokens) =
        match turn_context.config.model_auto_compact_token_limit_scope {
            AutoCompactTokenLimitScope::Total => (
                active_context_tokens,
                explicit_auto_compact_token_limit
                    .or(model_auto_compact_token_limit)
                    .or(token_budget_context_limit),
                None,
            ),
            AutoCompactTokenLimitScope::BodyAfterPrefix => {
                let window = sess.auto_compact_window_snapshot().await;
                let baseline = window.prefill_input_tokens.unwrap_or(active_context_tokens);

                let scope_limit =
                    explicit_auto_compact_token_limit.or(model_auto_compact_token_limit);
                (
                    active_context_tokens.saturating_sub(baseline),
                    scope_limit,
                    window.prefill_input_tokens,
                )
            }
        };

    // The model's full context window is a hard cap, independent of the auto-compaction scope.
    let full_context_window_limit = turn_context.model_context_window();

    // Report remaining tokens against the base (unbuffered) window, capped by the full context.
    let base_window_tokens_remaining = [
        tokens_remaining(auto_compact_scope_limit, auto_compact_scope_tokens),
        tokens_remaining(full_context_window_limit, active_context_tokens),
    ]
    .into_iter()
    .flatten()
    .min();

    // Only reserve the fallback buffer when there is a fallback prompt to use it.
    let auto_compact_fallback_buffer_tokens = turn_context
        .config
        .token_budget
        .as_ref()
        .map_or(0, crate::config::TokenBudgetConfig::fallback_buffer_tokens);
    let buffered_auto_compact_limit = auto_compact_scope_limit
        .map(|limit| limit.saturating_add(auto_compact_fallback_buffer_tokens));

    // Force compaction once the buffered window or the model's full context window is reached.
    let full_context_window_limit_reached =
        full_context_window_limit.is_some_and(|limit| active_context_tokens >= limit);
    let token_limit_reached = buffered_auto_compact_limit
        .is_some_and(|limit| auto_compact_scope_tokens >= limit)
        || full_context_window_limit_reached;

    ContextWindowTokenStatus {
        active_context_tokens,
        auto_compact_scope_tokens,
        auto_compact_scope_limit,
        full_context_window_limit,
        base_window_tokens_remaining,
        auto_compact_window_prefill_tokens,
        full_context_window_limit_reached,
        token_limit_reached,
    }
}

async fn recorded_context_tokens_for_context_window(
    sess: &Session,
    turn_context: &TurnContext,
) -> i64 {
    // Responses local estimates are diagnostic supplements to server accounting. Chat local
    // estimates are the explicit fallback when a compatible stream omits final usage, so they
    // remain authoritative for auto-compaction.
    let should_ignore_local_estimate = turn_context.provider.info().wire_api == WireApi::Responses
        && sess.token_usage_info().await.is_some_and(|info| {
            info.context_source == Some(ContextTokenUsageSource::LocalEstimate)
        });
    if should_ignore_local_estimate {
        sess.get_total_token_usage_without_context_tokens()
            .await
            .max(0)
    } else {
        sess.get_total_token_usage().await.max(0)
    }
}
