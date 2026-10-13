//! 最终失败必须持久化停止目标；单轮传输重试不能被空闲续跑重置。
use super::GoalExtensionHarness;
use super::seed_thread_metadata;
use super::test_runtime;
use super::test_thread_id;
use super::tool_by_name;
use super::tool_call;
use codex_extension_api::ExtensionData;
use codex_extension_api::TurnErrorInput;
use codex_protocol::error::CodexErr;
use codex_protocol::protocol::CodexErrorInfo;
use codex_protocol::protocol::TokenUsage;
use pretty_assertions::assert_eq;
use serde_json::json;

#[tokio::test]
async fn terminal_provider_errors_persist_a_non_active_goal() -> anyhow::Result<()> {
    let errors = [
        CodexErrorInfo::HttpConnectionFailed {
            http_status_code: Some(403),
        },
        CodexErrorInfo::HttpConnectionFailed {
            http_status_code: Some(401),
        },
        CodexErrorInfo::HttpConnectionFailed {
            http_status_code: Some(503),
        },
        CodexErrorInfo::HttpConnectionFailed {
            http_status_code: None,
        },
        CodexErrorInfo::ResponseTooManyFailedAttempts {
            http_status_code: Some(429),
        },
        CodexErrorInfo::ResponseStreamConnectionFailed {
            http_status_code: None,
        },
        CodexErrorInfo::ResponseStreamDisconnected {
            http_status_code: None,
        },
        CodexErrorInfo::ContextWindowExceeded,
        CodexErrorInfo::ServerOverloaded,
        CodexErrorInfo::InternalServerError,
        CodexErrorInfo::UsageLimitExceeded,
    ];
    for error in errors {
        let runtime = test_runtime().await?;
        let thread_id = test_thread_id()?;
        seed_thread_metadata(runtime.as_ref(), thread_id).await?;
        let harness = GoalExtensionHarness::new(runtime.clone(), thread_id).await?;
        harness
            .start_turn("turn-failed", &TokenUsage::default())
            .await;
        tool_by_name(&harness.tools(), "create_goal")
            .handle(tool_call(
                "create_goal",
                "create",
                json!({"objective": "finish fixture task"}),
            ))
            .await?;
        let turn_store = ExtensionData::new("turn-failed");
        let details = CodexErr::Fatal("fixture terminal failure".to_string());
        for contributor in harness.registry.turn_lifecycle_contributors() {
            contributor
                .on_turn_error(TurnErrorInput {
                    turn_id: "turn-failed",
                    error: error.clone(),
                    error_details: details.details(),
                    session_store: &harness.session_store,
                    thread_store: &harness.thread_store,
                    turn_store: &turn_store,
                })
                .await;
        }
        // 重复结束通知和重新装载 runtime 都不能把持久化停止状态变回 Active。
        harness.stop_turn("turn-failed").await;
        let restored = GoalExtensionHarness::new(runtime.clone(), thread_id).await?;
        restored
            .runtime_handle()
            .restore_after_resume()
            .await
            .map_err(anyhow::Error::msg)?;
        let goal = runtime
            .thread_goals()
            .get_thread_goal(thread_id)
            .await?
            .expect("goal exists");
        let expected = if error == CodexErrorInfo::UsageLimitExceeded {
            codex_state::ThreadGoalStatus::UsageLimited
        } else {
            codex_state::ThreadGoalStatus::Blocked
        };
        assert_eq!(goal.status, expected, "{error:?}");
        assert_eq!(goal.objective, "finish fixture task");
    }
    Ok(())
}
