use anyhow::Context;
use anyhow::Result;
use app_test_support::MockResponsesConfig;
use app_test_support::TestAppServer;
use app_test_support::create_final_assistant_message_sse_response;
use codex_app_server_protocol::ThreadGoalGetResponse;
use codex_app_server_protocol::ThreadGoalStatus;
use codex_app_server_protocol::ThreadResumeParams;
use codex_app_server_protocol::ThreadResumeResponse;
use codex_app_server_protocol::ThreadStartParams;
use codex_app_server_protocol::TurnStartParams;
use codex_app_server_protocol::TurnStatus;
use codex_app_server_protocol::UserInput;
use codex_features::Feature;
use core_test_support::responses;
use core_test_support::skip_if_no_network;
use pretty_assertions::assert_eq;
use serde_json::json;
use tempfile::TempDir;
use test_case::test_case;
use tokio::time::Duration;
use tokio::time::timeout;

enum FollowUpMode {
    Loaded,
    Resumed,
}

#[test_case(FollowUpMode::Loaded; "loaded_thread")]
#[test_case(FollowUpMode::Resumed; "cold_resume")]
#[tokio::test]
async fn grok_budget_limited_goal_allows_new_user_tool_turn(mode: FollowUpMode) -> Result<()> {
    skip_if_no_network!(Ok(()));
    let server = responses::start_mock_server().await;
    let response_mock = responses::mount_sse_sequence(
        &server,
        vec![
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
                responses::ev_response_created("check-stored-goal"),
                responses::ev_function_call("new-goal-call", "get_goal", "{}"),
                responses::ev_completed_with_tokens("check-stored-goal", /*total_tokens*/ 10),
            ]),
            responses::sse(vec![
                responses::ev_response_created("execute-new-request"),
                responses::ev_function_call(
                    "new-exec-call",
                    "exec_command",
                    &json!({
                        "cmd": "echo goal-follow-up-ok",
                        "timeout_ms": 30_000,
                    })
                    .to_string(),
                ),
                responses::ev_completed("execute-new-request"),
            ]),
            create_final_assistant_message_sse_response("The new request completed.")?,
            responses::sse(vec![
                responses::ev_response_created("check-goal-again"),
                responses::ev_function_call("second-goal-call", "get_goal", "{}"),
                responses::ev_completed_with_tokens("check-goal-again", /*total_tokens*/ 10),
            ]),
            create_final_assistant_message_sse_response("The second request completed.")?,
        ],
    )
    .await;
    let codex_home = TempDir::new()?;
    MockResponsesConfig::new(&server.uri())
        .with_model("grok-4.5")
        .with_sandbox_mode("danger-full-access")
        .enable_feature(Feature::Goals)
        .disable_feature(Feature::ShellSnapshot)
        .write(codex_home.path())?;
    std::fs::write(
        codex_home.path().join("requirements.toml"),
        "[features]\nunified_exec = false\nshell_tool = true\n",
    )?;
    let mut app = TestAppServer::builder()
        .with_codex_home(codex_home.path())
        .with_env_overrides(&[("CODEX_APP_SERVER_DISABLE_MANAGED_CONFIG", None)])
        .build_initialized()
        .await?;
    let thread = app.start_thread(ThreadStartParams::default()).await?.thread;
    let completed = timeout(
        Duration::from_secs(60),
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
    assert_eq!(response_mock.requests().len(), 2);
    let request_id = app
        .send_raw_request("thread/goal/get", Some(json!({"threadId": thread.id})))
        .await?;
    let before: ThreadGoalGetResponse = app.read_response(request_id).await?;
    let before = before.goal.context("goal must persist after exhaustion")?;
    assert_eq!(before.status, ThreadGoalStatus::BudgetLimited);
    assert!(before.tokens_used >= 25);

    if matches!(mode, FollowUpMode::Resumed) {
        app.shutdown_gracefully().await?;
        app = TestAppServer::builder()
            .with_codex_home(codex_home.path())
            .with_env_overrides(&[("CODEX_APP_SERVER_DISABLE_MANAGED_CONFIG", None)])
            .build_initialized()
            .await?;
        let request_id = app
            .send_thread_resume_request(ThreadResumeParams {
                thread_id: thread.id.clone(),
                ..Default::default()
            })
            .await?;
        let _: ThreadResumeResponse = app.read_response(request_id).await?;
    }

    let completed = timeout(
        Duration::from_secs(60),
        app.start_turn_and_wait_for_completion(TurnStartParams {
            thread_id: thread.id.clone(),
            input: vec![UserInput::Text {
                text: "Check the stored goal, then run a command for this new request.".to_string(),
                text_elements: Vec::new(),
            }],
            ..Default::default()
        }),
    )
    .await??;
    assert_eq!(completed.turn.status, TurnStatus::Completed);
    let requests = response_mock.requests();
    assert_eq!(requests.len(), 5);
    for request in &requests[..2] {
        let context = request.message_input_texts("developer").join("\n");
        assert_eq!(context.matches("not a budget for this new turn").count(), 0);
    }
    for request in &requests[2..] {
        let context = request.message_input_texts("developer").join("\n");
        assert_eq!(context.matches("not a budget for this new turn").count(), 1);
        assert!(context.contains("Do not automatically continue the stopped goal"));
    }
    let exec_tool = requests[3]
        .tool_by_name("functions", "exec_command")
        .or_else(|| {
            requests[3].body_json()["tools"]
                .as_array()?
                .iter()
                .find(|tool| tool["name"] == "exec_command")
                .cloned()
        })
        .context("exec_command must be registered for the selected model and environment")?;
    let exec_parameters = exec_tool["parameters"]["properties"]
        .as_object()
        .context("exec_command must expose parameters")?;
    assert!(exec_parameters.contains_key("timeout_ms"));
    assert!(!exec_parameters.contains_key("yield_time_ms"));
    let goal_output: serde_json::Value = serde_json::from_str(
        &requests[3]
            .function_call_output_text("new-goal-call")
            .context("get_goal should execute in the new turn")?,
    )?;
    assert_eq!(goal_output["goal"]["status"], json!("budgetLimited"));
    assert_eq!(goal_output["remainingTokens"], json!(0));
    let output = requests[4]
        .function_call_output_text("new-exec-call")
        .context("exec_command should execute in the new turn")?;
    assert!(
        output.contains("Process exited with code 0"),
        "unexpected command output: {output}"
    );
    assert!(
        output
            .lines()
            .any(|line| line.trim() == "goal-follow-up-ok"),
        "unexpected command output: {output}"
    );
    let request_id = app
        .send_raw_request("thread/goal/get", Some(json!({"threadId": thread.id})))
        .await?;
    let after: ThreadGoalGetResponse = app.read_response(request_id).await?;
    assert_eq!(after.goal, Some(before.clone()));

    let completed = timeout(
        Duration::from_secs(60),
        app.start_turn_and_wait_for_completion(TurnStartParams {
            thread_id: thread.id.clone(),
            input: vec![UserInput::Text {
                text: "Check the stored goal again for this separate request.".to_string(),
                text_elements: Vec::new(),
            }],
            ..Default::default()
        }),
    )
    .await??;
    assert_eq!(completed.turn.status, TurnStatus::Completed);
    let requests = response_mock.requests();
    assert_eq!(requests.len(), 7);
    for request in &requests[5..] {
        let context = request.message_input_texts("developer").join("\n");
        assert_eq!(context.matches("not a budget for this new turn").count(), 2);
    }
    for consecutive_requests in requests[2..].windows(2) {
        let previous_input = consecutive_requests[0].input();
        let next_input = consecutive_requests[1].input();
        assert_eq!(
            next_input.get(..previous_input.len()),
            Some(previous_input.as_slice()),
            "later requests must preserve the complete earlier input prefix"
        );
    }
    let goal_output: serde_json::Value = serde_json::from_str(
        &requests[6]
            .function_call_output_text("second-goal-call")
            .context("get_goal should execute in the second ordinary turn")?,
    )?;
    assert_eq!(goal_output["goal"]["status"], json!("budgetLimited"));
    assert_eq!(goal_output["remainingTokens"], json!(0));
    let request_id = app
        .send_raw_request("thread/goal/get", Some(json!({"threadId": thread.id})))
        .await?;
    let after: ThreadGoalGetResponse = app.read_response(request_id).await?;
    assert_eq!(after.goal, Some(before));
    Ok(())
}
