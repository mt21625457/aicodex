use anyhow::Context;
use anyhow::Result;
use app_test_support::MockResponsesConfig;
use app_test_support::TestAppServer;
use app_test_support::create_final_assistant_message_sse_response;
use codex_app_server_protocol::ThreadGoalGetResponse;
use codex_app_server_protocol::ThreadGoalSetResponse;
use codex_app_server_protocol::ThreadGoalStatus;
use codex_app_server_protocol::ThreadStartParams;
use codex_app_server_protocol::TurnCompletedNotification;
use codex_app_server_protocol::TurnStartParams;
use codex_app_server_protocol::TurnStartResponse;
use codex_app_server_protocol::TurnStatus;
use codex_app_server_protocol::UserInput;
use codex_features::Feature;
use core_test_support::responses;
use core_test_support::skip_if_no_network;
use core_test_support::streaming_sse::StreamingSseChunk;
use core_test_support::streaming_sse::start_streaming_sse_server;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;
use tempfile::TempDir;
use tokio::sync::oneshot;
use tokio::time::Duration;
use tokio::time::timeout;

#[tokio::test]
async fn same_objective_resume_appends_context_during_user_turn() -> Result<()> {
    skip_if_no_network!(Ok(()));
    let resume_marker = "The user resumed the stored goal. Its status is now active.";
    let wait = Duration::from_secs(60);
    let (release_response, response_gate) = oneshot::channel();
    let mut response_gate = Some(response_gate);
    let streams = vec![
        responses::sse(vec![
            responses::ev_response_created("create-goal"),
            responses::ev_function_call(
                "create-goal-call",
                "create_goal",
                r#"{"objective":"finish the task","token_budget":25}"#,
            ),
            responses::ev_completed_with_tokens("create-goal", /*total_tokens*/ 1),
        ]),
        responses::sse(vec![
            responses::ev_response_created("exhaust-budget"),
            responses::ev_function_call("old-goal-call", "get_goal", "{}"),
            responses::ev_completed_with_tokens("exhaust-budget", /*total_tokens*/ 30),
        ]),
        responses::sse(vec![
            responses::ev_response_created("new-request"),
            responses::ev_function_call("new-goal-call", "get_goal", "{}"),
            responses::ev_completed_with_tokens("new-request", /*total_tokens*/ 10),
        ]),
        responses::sse(vec![
            responses::ev_response_created("complete-goal"),
            responses::ev_function_call(
                "complete-goal-call",
                "update_goal",
                r#"{"status":"complete"}"#,
            ),
            responses::ev_completed("complete-goal"),
        ]),
        create_final_assistant_message_sse_response("The resumed goal is complete.")?,
    ]
    .into_iter()
    .enumerate()
    .map(|(index, body)| {
        vec![StreamingSseChunk {
            gate: if index == 2 {
                response_gate.take()
            } else {
                None
            },
            body,
        }]
    })
    .collect();
    let (server, _completions) = start_streaming_sse_server(streams).await;
    let codex_home = TempDir::new()?;
    MockResponsesConfig::new(server.uri())
        .with_model("grok-4.5")
        .enable_feature(Feature::Goals)
        .disable_feature(Feature::ShellSnapshot)
        .write(codex_home.path())?;
    let mut app = TestAppServer::builder()
        .with_codex_home(codex_home.path())
        .without_managed_config()
        .build_initialized()
        .await?;
    let thread = app.start_thread(ThreadStartParams::default()).await?.thread;
    let completed = timeout(
        wait,
        app.start_turn_and_wait_for_completion(TurnStartParams {
            thread_id: thread.id.clone(),
            input: vec![UserInput::Text {
                text: "Create a goal to finish the task with a 25 token budget.".to_string(),
                text_elements: Vec::new(),
            }],
            ..Default::default()
        }),
    )
    .await??;
    assert_eq!(completed.turn.status, TurnStatus::Interrupted);
    let request_id = app
        .send_raw_request("thread/goal/get", Some(json!({"threadId": thread.id})))
        .await?;
    let before: ThreadGoalGetResponse = app.read_response(request_id).await?;
    let before = before.goal.context("exhausted goal should persist")?;
    assert_eq!(before.status, ThreadGoalStatus::BudgetLimited);
    assert!(before.tokens_used >= 25);

    let request_id = app
        .send_turn_start_request(TurnStartParams {
            thread_id: thread.id.clone(),
            input: vec![UserInput::Text {
                text: "Check the goal while handling this new request.".to_string(),
                text_elements: Vec::new(),
            }],
            ..Default::default()
        })
        .await?;
    let started: TurnStartResponse = app.read_response(request_id).await?;
    timeout(wait, server.wait_for_request_count(/*count*/ 3)).await?;
    let requests = server.requests().await;
    let gated: Value = serde_json::from_slice(&requests[2])?;
    let old_context = gated["input"]
        .as_array()
        .context("request input")?
        .iter()
        .find(|item| {
            item["role"] == "developer"
                && item["content"]
                    .to_string()
                    .contains("not a budget for this new turn")
        })
        .context("new turn should contain the stopped-goal snapshot")?;
    let old_context = old_context["content"]
        .as_array()
        .context("developer content")?
        .iter()
        .filter_map(|content| content["text"].as_str())
        .find(|text| text.contains("not a budget for this new turn"))
        .context("stopped-goal snapshot text")?;
    assert!(!gated["input"].to_string().contains(resume_marker));

    let request_id = app
        .send_raw_request(
            "thread/goal/set",
            Some(json!({
                "threadId": thread.id, "status": "active", "tokenBudget": 250_000,
            })),
        )
        .await?;
    let resumed: ThreadGoalSetResponse = app.read_response(request_id).await?;
    assert_eq!(
        (
            &resumed.goal.objective,
            resumed.goal.status,
            resumed.goal.tokens_used,
            resumed.goal.token_budget
        ),
        (
            &before.objective,
            ThreadGoalStatus::Active,
            before.tokens_used,
            Some(250_000)
        ),
    );
    release_response
        .send(())
        .expect("sampling gate should remain open");
    let completed: TurnCompletedNotification =
        timeout(wait, app.read_notification("turn/completed")).await??;
    assert_eq!(
        (completed.turn.id, completed.turn.status),
        (started.turn.id, TurnStatus::Completed)
    );
    let requests = server.requests().await;
    assert_eq!(requests.len(), 5);
    let follow_up: Value = serde_json::from_slice(&requests[3])?;
    let input = follow_up["input"].as_array().context("follow-up input")?;
    assert!(
        input
            .iter()
            .filter(|item| item["role"] == "developer")
            .filter_map(|item| item["content"].as_array())
            .flatten()
            .any(|content| content["text"].as_str() == Some(old_context)),
        "old context text must remain unchanged"
    );
    let resumed_context = input
        .iter()
        .filter_map(|item| item["content"].as_array())
        .flatten()
        .filter_map(|content| content["text"].as_str())
        .find(|text| text.contains(resume_marker))
        .context("next request must include the user-controlled resume")?;
    let tokens_used = resumed.goal.tokens_used;
    let token_budget = resumed.goal.token_budget.context("resumed budget")?;
    let remaining_tokens = token_budget - tokens_used;
    for expected in [
        format!("Tokens used: {tokens_used}"),
        format!("Token budget: {token_budget}"),
        format!("Tokens remaining: {remaining_tokens}"),
    ] {
        assert!(
            resumed_context.contains(&expected),
            "missing {expected}: {resumed_context}"
        );
    }
    let request_id = app
        .send_raw_request("thread/goal/get", Some(json!({"threadId": thread.id})))
        .await?;
    let after: ThreadGoalGetResponse = app.read_response(request_id).await?;
    assert_eq!(
        after.goal.context("completed goal")?.status,
        ThreadGoalStatus::Complete
    );
    app.shutdown_gracefully().await?;
    server.shutdown().await;
    Ok(())
}
