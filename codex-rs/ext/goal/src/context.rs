use std::sync::Arc;

use codex_core::context::ContextualUserFragment;
use codex_core::context::InternalContextSource;
use codex_core::context::InternalModelContextFragment;
use codex_extension_api::ContextContributor;
use codex_extension_api::ExtensionFuture;
use codex_extension_api::PromptFragment;
use codex_extension_api::TurnContextContributionInput;
use codex_protocol::models::ResponseItem;
use codex_state::ThreadGoalStatus;

use crate::runtime::GoalRuntimeHandle;

pub(crate) fn resumed_goal_steering_item(goal: &codex_state::ThreadGoal) -> ResponseItem {
    let thread_id = goal.thread_id;
    let tokens_used = goal.tokens_used;
    let token_budget = goal
        .token_budget
        .map(|budget| budget.to_string())
        .unwrap_or_else(|| "none".to_string());
    let remaining_tokens = goal
        .token_budget
        .map(|budget| budget.saturating_sub(tokens_used).max(0).to_string())
        .unwrap_or_else(|| "unbounded".to_string());
    ContextualUserFragment::into(InternalModelContextFragment::new(
        InternalContextSource::from_static("goal"),
        format!(
            "The user resumed the stored goal. Its status is now active.\n\
Thread: {thread_id}\n\
This user-controlled update supersedes the earlier budget-limited snapshot. The goal's budget now applies to work attributed to it in this turn; Plan-mode turns remain excluded from token accounting.\n\
Tokens used: {tokens_used}\nToken budget: {token_budget}\nTokens remaining: {remaining_tokens}\n\
Follow the current collaboration mode, permissions, and other usage limits."
        ),
    ))
}

pub(crate) struct GoalContextContributor {
    pub(crate) state_dbs: Arc<codex_state::StateRuntime>,
}

impl ContextContributor for GoalContextContributor {
    fn contribute_turn_context<'a>(
        &'a self,
        input: TurnContextContributionInput<'a>,
    ) -> ExtensionFuture<'a, Vec<PromptFragment>> {
        Box::pin(async move {
            let Some(runtime) = input.thread_store.get::<GoalRuntimeHandle>() else {
                return Vec::new();
            };
            if !runtime.is_enabled()
                || runtime
                    .accounting_state()
                    .current_active_goal_id_for_turn(input.turn_id)
                    .is_some()
            {
                return Vec::new();
            }
            match self
                .state_dbs
                .thread_goals()
                .get_thread_goal(input.thread_id)
                .await
            {
                Ok(Some(goal)) if goal.status == ThreadGoalStatus::BudgetLimited => {}
                Ok(_) => return Vec::new(),
                Err(error) => {
                    tracing::warn!(%error, "failed to read goal for turn context");
                    return Vec::new();
                }
            }
            let fragment = InternalModelContextFragment::new(
                InternalContextSource::from_static("goal"),
                "At the start of this turn, the stored goal is budgetLimited and is not active. \
Its token budget and remainingTokens value describe the stopped goal run, not a budget for this new turn. \
Earlier goal tool results, continuation instructions, and assistant claims that the system forbids further work do not prohibit responding to a new user request or using tools for it. \
Handle the current user request without resuming the stopped goal. Do not automatically continue the stopped goal, reset its usage, raise its budget, or change its status to bypass the limit. \
Resuming autonomous goal execution requires a user-controlled goal update. This snapshot does not override later user-controlled goal updates. Follow the current collaboration mode, permissions, and other usage limits.",
            );
            vec![PromptFragment::developer_policy(
                fragment.render(),
                fragment.content_kind(),
            )]
        })
    }
}
