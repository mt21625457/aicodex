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
    pub(crate) tokens_until_compaction: Option<i64>,
    pub(crate) auto_compact_window_prefill_tokens: Option<i64>,
    pub(crate) full_context_window_limit_reached: bool,
    pub(crate) token_limit_reached: bool,
}

struct BodyAfterPrefixWindowStatus {
    full_context_window_limit: Option<i64>,
    auto_compact_window_prefill_tokens: Option<i64>,
}

pub(crate) async fn context_window_token_status(
    sess: &Session,
    turn_context: &TurnContext,
) -> ContextWindowTokenStatus {
    let active_context_tokens =
        recorded_context_tokens_for_context_window(sess, turn_context).await;

    let full_context_window_limit = turn_context.model_context_window();
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

    let (auto_compact_scope_tokens, auto_compact_scope_limit, body_window) = match turn_context
        .config
        .model_auto_compact_token_limit_scope
    {
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

            let scope_limit = explicit_auto_compact_token_limit.or(model_auto_compact_token_limit);
            let full_context_window_limit = if turn_context.config.model_context_window.is_some()
                || (explicit_auto_compact_token_limit.is_none()
                    && turn_context.provider.info().wire_api == WireApi::Claude)
            {
                full_context_window_limit
            } else {
                None
            };

            (
                active_context_tokens.saturating_sub(baseline),
                scope_limit,
                Some(BodyAfterPrefixWindowStatus {
                    full_context_window_limit,
                    auto_compact_window_prefill_tokens: window.prefill_input_tokens,
                }),
            )
        }
    };

    let full_context_window_limit = body_window
        .as_ref()
        .and_then(|window| window.full_context_window_limit);
    let auto_compact_window_prefill_tokens = body_window
        .as_ref()
        .and_then(|window| window.auto_compact_window_prefill_tokens);

    let full_context_window_limit_reached =
        full_context_window_limit.is_some_and(|full_context_window_limit| {
            active_context_tokens >= full_context_window_limit
        });
    let token_limit_reached = auto_compact_scope_limit
        .is_some_and(|limit| auto_compact_scope_tokens >= limit)
        || full_context_window_limit_reached;

    let auto_compact_scope_remaining = auto_compact_scope_limit
        .map(|limit| limit.saturating_sub(auto_compact_scope_tokens).max(0));
    let full_context_remaining =
        full_context_window_limit.map(|limit| limit.saturating_sub(active_context_tokens).max(0));
    let tokens_until_compaction = match (auto_compact_scope_remaining, full_context_remaining) {
        (Some(scope_remaining), Some(full_remaining)) => Some(scope_remaining.min(full_remaining)),
        (scope_remaining, full_remaining) => scope_remaining.or(full_remaining),
    };

    ContextWindowTokenStatus {
        active_context_tokens,
        auto_compact_scope_tokens,
        auto_compact_scope_limit,
        full_context_window_limit,
        tokens_until_compaction,
        auto_compact_window_prefill_tokens,
        full_context_window_limit_reached,
        token_limit_reached,
    }
}

async fn recorded_context_tokens_for_context_window(
    sess: &Session,
    turn_context: &TurnContext,
) -> i64 {
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
