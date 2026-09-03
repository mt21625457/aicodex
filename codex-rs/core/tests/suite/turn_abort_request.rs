use std::sync::Arc;

use anyhow::Result;
use codex_core::TurnInputRequest;
use codex_core::config::Config;
use codex_extension_api::ExtensionFuture;
use codex_extension_api::ExtensionRegistryBuilder;
use codex_extension_api::ThreadIdleCause;
use codex_extension_api::ThreadIdleInput;
use codex_extension_api::ThreadLifecycleContributor;
use codex_extension_api::ToolFinishInput;
use codex_extension_api::ToolLifecycleContributor;
use codex_extension_api::ToolLifecycleFuture;
use codex_extension_api::TurnAbortRequest;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::TurnAbortReason;
use codex_protocol::user_input::UserInput;
use core_test_support::responses;
use core_test_support::test_codex::test_codex;
use core_test_support::wait_for_event;
use pretty_assertions::assert_eq;

struct BudgetLimitAfterTool;

struct IdleCauseRecorder(async_channel::Sender<ThreadIdleCause>);

impl ThreadLifecycleContributor<Config> for IdleCauseRecorder {
    fn on_thread_idle<'a>(&'a self, input: ThreadIdleInput<'a>) -> ExtensionFuture<'a, ()> {
        Box::pin(async move {
            self.0
                .send(input.cause)
                .await
                .expect("idle cause receiver should remain open");
        })
    }
}

impl ToolLifecycleContributor for BudgetLimitAfterTool {
    fn on_tool_finish<'a>(&'a self, input: ToolFinishInput<'a>) -> ToolLifecycleFuture<'a> {
        Box::pin(async move {
            input
                .turn_store
                .insert(TurnAbortRequest::new(TurnAbortReason::BudgetLimited));
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn extension_abort_request_stops_before_follow_up_and_preserves_reason() -> Result<()> {
    let server = responses::start_mock_server().await;
    let response_mock = responses::mount_sse_sequence(
        &server,
        vec![responses::sse(vec![
            responses::ev_response_created("tool-response"),
            responses::ev_function_call(
                "update-plan-call",
                "update_plan",
                r#"{"plan":[{"step":"stop after this tool","status":"completed"}]}"#,
            ),
            responses::ev_completed("tool-response"),
        ])],
    )
    .await;
    let mut extensions = ExtensionRegistryBuilder::<Config>::new();
    extensions.tool_lifecycle_contributor(Arc::new(BudgetLimitAfterTool));
    let (idle_cause_tx, idle_cause_rx) = async_channel::bounded(1);
    extensions.thread_lifecycle_contributor(Arc::new(IdleCauseRecorder(idle_cause_tx)));
    let test = test_codex()
        .with_extensions(Arc::new(extensions.build()))
        .with_config(|config| config.update_plan_enabled = true)
        .build_with_auto_env(&server)
        .await?;

    test.codex
        .start_or_steer_turn(TurnInputRequest::user_input(vec![UserInput::Text {
            text: "Update the plan once.".to_string(),
            text_elements: Vec::new(),
        }]))
        .await?;

    let EventMsg::TurnAborted(aborted) = wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnAborted(_))
    })
    .await
    else {
        unreachable!();
    };
    assert_eq!(aborted.reason, TurnAbortReason::BudgetLimited);
    assert_eq!(response_mock.requests().len(), 1);
    assert_eq!(idle_cause_rx.recv().await?, ThreadIdleCause::BudgetLimited);

    Ok(())
}
