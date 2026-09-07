use super::GoalExtensionHarness;
use super::input_token_usage;
use super::seed_thread_metadata;
use super::test_runtime;
use super::test_thread_id;
use super::tool_by_name;
use super::tool_call;
use codex_extension_api::ExtensionData;
use codex_extension_api::PromptFragment;
use codex_extension_api::TurnContextContributionInput;
use codex_goal_extension::GoalExtensionConfig;
use codex_protocol::ThreadId;
use codex_protocol::config_types::ModeKind;
use codex_protocol::protocol::TokenUsage;
use codex_state::ThreadGoalStatus;
use pretty_assertions::assert_eq;
use serde_json::json;

async fn context_for_turn(harness: &GoalExtensionHarness, turn_id: &str) -> Vec<PromptFragment> {
    let turn_store = ExtensionData::new(turn_id);
    let mut fragments = Vec::new();
    for contributor in harness.registry.context_contributors() {
        fragments.extend(
            contributor
                .contribute_turn_context(TurnContextContributionInput {
                    thread_id: ThreadId::from_string(harness.thread_store.level_id())
                        .expect("valid thread id"),
                    turn_id,
                    session_store: &harness.session_store,
                    thread_store: &harness.thread_store,
                    turn_store: &turn_store,
                    model_context_window: None,
                })
                .await,
        );
    }
    fragments
}

#[tokio::test]
async fn budget_context_follows_persisted_status_without_changing_goal() -> anyhow::Result<()> {
    let runtime = test_runtime().await?;
    let thread_id = test_thread_id()?;
    seed_thread_metadata(runtime.as_ref(), thread_id).await?;
    let harness = GoalExtensionHarness::new(runtime.clone(), thread_id).await?;
    assert_eq!(context_for_turn(&harness, "empty").await, Vec::new());

    for (status, budget, mode) in [
        (ThreadGoalStatus::Active, Some(250_000), ModeKind::Default),
        (ThreadGoalStatus::Paused, Some(250_000), ModeKind::Default),
        (ThreadGoalStatus::Blocked, Some(250_000), ModeKind::Default),
        (
            ThreadGoalStatus::UsageLimited,
            Some(250_000),
            ModeKind::Default,
        ),
        (ThreadGoalStatus::Complete, Some(250_000), ModeKind::Default),
        (
            ThreadGoalStatus::BudgetLimited,
            Some(250_000),
            ModeKind::Default,
        ),
        (ThreadGoalStatus::BudgetLimited, None, ModeKind::Default),
        (
            ThreadGoalStatus::BudgetLimited,
            Some(250_000),
            ModeKind::Plan,
        ),
    ] {
        let goal = runtime
            .thread_goals()
            .replace_thread_goal(thread_id, "persisted goal", status, budget)
            .await?;
        harness
            .start_turn_with_mode("next-turn", mode, &TokenUsage::default())
            .await;
        let fragments = context_for_turn(&harness, "next-turn").await;
        if status == ThreadGoalStatus::BudgetLimited {
            let invocation = tool_call("get_goal", "call-get-goal", json!({}));
            let output = tool_by_name(&harness.tools(), "get_goal")
                .handle(invocation.clone())
                .await?;
            let result = output.code_mode_result(&invocation.payload);
            assert_eq!(
                (
                    result["goal"]["status"].clone(),
                    result["remainingTokens"].clone()
                ),
                (json!("budgetLimited"), json!(budget))
            );
            assert_eq!(fragments.len(), 1);
            assert!(
                fragments[0]
                    .text()
                    .contains("not a budget for this new turn")
            );
            assert!(fragments[0].text().len() < 1_000);
            assert!(fragments[0].text().contains(
                "Its token budget and remainingTokens value describe the stopped goal run"
            ));
            assert!(
                fragments[0]
                    .text()
                    .contains("Follow the current collaboration mode")
            );
        } else {
            assert_eq!(fragments, Vec::new());
        }
        assert_eq!(
            runtime.thread_goals().get_thread_goal(thread_id).await?,
            Some(goal)
        );
        harness.stop_turn("next-turn").await;
    }
    let goal = runtime.thread_goals().get_thread_goal(thread_id).await?;
    let disabled = GoalExtensionHarness::with_config(
        runtime.clone(),
        thread_id,
        GoalExtensionConfig {
            enabled: false,
            max_goal_token_budget: None,
        },
    )
    .await?;
    disabled
        .start_turn("disabled-turn", &TokenUsage::default())
        .await;
    assert_eq!(
        context_for_turn(&disabled, "disabled-turn").await,
        Vec::new()
    );
    assert_eq!(
        runtime.thread_goals().get_thread_goal(thread_id).await?,
        goal
    );
    Ok(())
}

#[tokio::test]
async fn budget_context_survives_resume_but_does_not_release_the_exhausting_turn()
-> anyhow::Result<()> {
    let runtime = test_runtime().await?;
    let thread_id = test_thread_id()?;
    seed_thread_metadata(runtime.as_ref(), thread_id).await?;
    runtime
        .thread_goals()
        .replace_thread_goal(
            thread_id,
            "stop at the budget",
            codex_state::ThreadGoalStatus::Active,
            Some(25),
        )
        .await?;
    let harness = GoalExtensionHarness::new(runtime.clone(), thread_id).await?;
    harness
        .start_turn("exhausting-turn", &TokenUsage::default())
        .await;
    harness
        .record_token_usage("exhausting-turn", &input_token_usage(/*input_tokens*/ 30))
        .await;
    harness
        .notify_tool_finish("exhausting-turn", "call-1", "shell")
        .await;
    assert_eq!(
        context_for_turn(&harness, "exhausting-turn").await,
        Vec::new()
    );
    harness.stop_turn("exhausting-turn").await;
    let mut goal = runtime
        .thread_goals()
        .get_thread_goal(thread_id)
        .await?
        .expect("stored goal");
    goal.token_budget = Some(250_000);
    goal.tokens_used = 1_450_000;
    runtime
        .thread_goals()
        .replace_thread_goal_snapshot(&goal)
        .await?;

    let resumed = GoalExtensionHarness::new(runtime.clone(), thread_id).await?;
    resumed.resume_thread().await;
    resumed
        .start_turn("user-turn", &input_token_usage(/*input_tokens*/ 1_450_000))
        .await;
    let fragments = context_for_turn(&resumed, "user-turn").await;
    assert_eq!(fragments.len(), 1);
    assert!(
        fragments[0]
            .text()
            .contains("not a budget for this new turn")
    );
    resumed
        .record_token_usage("user-turn", &input_token_usage(/*input_tokens*/ 1_450_010))
        .await;
    resumed
        .notify_tool_finish("user-turn", "call-2", "shell")
        .await;
    resumed.stop_turn("user-turn").await;
    assert_eq!(
        runtime.thread_goals().get_thread_goal(thread_id).await?,
        Some(goal)
    );

    Ok(())
}
