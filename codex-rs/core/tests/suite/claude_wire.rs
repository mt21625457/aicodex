use codex_core::config::Config;
use codex_model_provider_info::WireApi;
use codex_protocol::models::PermissionProfile;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::Op;
use codex_protocol::protocol::ReviewDecision;
use codex_protocol::protocol::SandboxPolicy;
use codex_protocol::user_input::UserInput;
use core_test_support::responses::ev_assistant_message;
use core_test_support::responses::ev_completed_with_tokens;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::mount_claude_count_tokens_never;
use core_test_support::responses::mount_claude_count_tokens_response;
use core_test_support::responses::mount_claude_sse_sequence;
use core_test_support::responses::mount_sse_sequence;
use core_test_support::responses::sse;
use core_test_support::responses::start_mock_server;
use core_test_support::test_codex::TestCodex;
use core_test_support::test_codex::test_codex;
use core_test_support::wait_for_event;
use core_test_support::wait_for_event_match;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;
use wiremock::ResponseTemplate;

const EXISTING_ENV_VAR_WITH_NON_EMPTY_VALUE: &str = "PATH";

async fn submit_turn_and_wait_for_token_event<F>(
    test: &TestCodex,
    prompt: &str,
    matches_token_event: F,
) -> anyhow::Result<EventMsg>
where
    F: Fn(&EventMsg) -> bool,
{
    test.codex
        .submit(Op::UserTurn {
            environments: None,
            items: vec![UserInput::Text {
                text: prompt.into(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            cwd: test.config.cwd.to_path_buf(),
            approval_policy: AskForApproval::Never,
            approvals_reviewer: None,
            sandbox_policy: SandboxPolicy::DangerFullAccess,
            permission_profile: Some(PermissionProfile::Disabled),
            model: test.session_configured.model.clone(),
            effort: None,
            summary: None,
            service_tier: None,
            collaboration_mode: None,
            personality: None,
        })
        .await?;

    let token_event = wait_for_event(&test.codex, matches_token_event).await;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;
    Ok(token_event)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn claude_wire_tool_loop_posts_messages_and_tool_result() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let responses = mount_claude_sse_sequence(
        &server,
        vec![claude_tool_use_sse(), claude_text_sse("msg_2", "done")],
    )
    .await;
    let count_tokens = mount_claude_count_tokens_response(
        &server,
        ResponseTemplate::new(200).set_body_json(json!({ "input_tokens": 789 })),
        1,
    )
    .await;

    let test = test_codex()
        .with_config(configure_claude_provider)
        .build(&server)
        .await?;

    test.submit_turn("run a shell command").await?;

    let requests = responses.requests();
    assert_eq!(requests.len(), 2);
    for request in &requests {
        assert_eq!(request.path(), "/v1/messages");
        assert_eq!(
            request.header("anthropic-version").as_deref(),
            Some("2023-06-01")
        );
        assert_eq!(
            request.header("accept").as_deref(),
            Some("text/event-stream")
        );
        assert!(request.header("x-api-key").is_some());
        assert_eq!(request.header("authorization"), None);
    }

    let first = requests[0].body_json();
    let first_tool_names = tool_names(&first);
    assert!(
        first_tool_names.iter().any(|name| name == "exec_command"),
        "first Claude request tools: {first_tool_names:?}"
    );

    let second = requests[1].body_json();
    let tool_result = message_content_blocks(&second)
        .into_iter()
        .find(|block| {
            block.get("type").and_then(Value::as_str) == Some("tool_result")
                && block.get("tool_use_id").and_then(Value::as_str) == Some("toolu_1")
        })
        .unwrap_or_else(|| panic!("second Claude request should include tool_result: {second}"));
    assert!(
        tool_result["content"]
            .as_str()
            .expect("tool_result content")
            .contains("claude")
    );

    let count_request = count_tokens.single_request().body_json();
    assert!(
        message_content_blocks(&count_request)
            .iter()
            .any(|block| block.get("text").and_then(Value::as_str) == Some("done")),
        "count_tokens should run after the Claude tool loop completes: {count_request}"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn claude_wire_uses_count_tokens_for_post_turn_context_usage() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let responses = mount_claude_sse_sequence(
        &server,
        vec![claude_text_sse_without_usage("msg_1", "done")],
    )
    .await;
    let count_tokens = mount_claude_count_tokens_response(
        &server,
        ResponseTemplate::new(200).set_body_json(json!({ "input_tokens": 456 })),
        1,
    )
    .await;

    let test = test_codex()
        .with_config(configure_claude_provider)
        .build(&server)
        .await?;

    let token_event =
        submit_turn_and_wait_for_token_event(&test, "count this Claude context", |event| {
            matches!(
                event,
                EventMsg::TokenCount(payload)
                    if payload.info.as_ref().is_some_and(|info| {
                        info.last_token_usage.total_tokens == 456
                            && info.last_token_usage.input_tokens == 456
                    })
            )
        })
        .await?;
    let EventMsg::TokenCount(payload) = token_event else {
        unreachable!("wait_for_event returned unexpected event");
    };
    assert_eq!(
        payload
            .info
            .expect("token usage info")
            .last_token_usage
            .total_tokens,
        456
    );

    let count_request = count_tokens.single_request().body_json();
    assert!(
        message_content_blocks(&count_request)
            .iter()
            .any(|block| block.get("text").and_then(Value::as_str) == Some("done")),
        "count_tokens request should include completed assistant context: {count_request}"
    );
    assert_eq!(responses.requests().len(), 1);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn claude_wire_fallback_uses_local_context_estimate() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    mount_claude_sse_sequence(
        &server,
        vec![claude_text_sse_without_usage("msg_1", "done")],
    )
    .await;
    mount_claude_count_tokens_response(
        &server,
        ResponseTemplate::new(404).set_body_json(json!({
            "error": {
                "type": "not_found_error",
                "message": "count_tokens is not supported"
            }
        })),
        1,
    )
    .await;

    let test = test_codex()
        .with_config(configure_claude_provider)
        .build(&server)
        .await?;

    let token_event = submit_turn_and_wait_for_token_event(
        &test,
        "count this Claude context with fallback",
        |event| {
            matches!(
                event,
                EventMsg::TokenCount(payload)
                    if payload.info.as_ref().is_some_and(|info| {
                        info.last_token_usage.total_tokens > 0
                    })
            )
        },
    )
    .await?;
    let EventMsg::TokenCount(payload) = token_event else {
        unreachable!("wait_for_event returned unexpected event");
    };
    assert!(
        payload
            .info
            .expect("token usage info")
            .last_token_usage
            .total_tokens
            > 0
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn responses_provider_does_not_call_claude_count_tokens() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let responses = mount_sse_sequence(
        &server,
        vec![sse(vec![
            ev_response_created("resp_1"),
            ev_assistant_message("msg_1", "done"),
            ev_completed_with_tokens("resp_1", 321),
        ])],
    )
    .await;
    let count_tokens = mount_claude_count_tokens_never(&server).await;

    let test = test_codex().build(&server).await?;

    let token_event = submit_turn_and_wait_for_token_event(
        &test,
        "keep Responses accounting unchanged",
        |event| {
            matches!(
                event,
                EventMsg::TokenCount(payload)
                    if payload.info.as_ref().is_some_and(|info| {
                        info.last_token_usage.total_tokens == 321
                    })
            )
        },
    )
    .await?;
    let EventMsg::TokenCount(payload) = token_event else {
        unreachable!("wait_for_event returned unexpected event");
    };
    assert_eq!(
        payload
            .info
            .expect("token usage info")
            .last_token_usage
            .total_tokens,
        321
    );

    assert_eq!(responses.requests().len(), 1);
    assert_eq!(count_tokens.requests().len(), 0);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn claude_wire_preserves_multiple_tool_results_in_order() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let responses = mount_claude_sse_sequence(
        &server,
        vec![
            claude_multi_tool_use_sse(),
            claude_text_sse("msg_2", "done"),
        ],
    )
    .await;

    let test = test_codex()
        .with_config(configure_claude_provider)
        .build(&server)
        .await?;

    test.submit_turn("run two shell commands").await?;

    let requests = responses.requests();
    assert_eq!(requests.len(), 2);

    let second = requests[1].body_json();
    let tool_results = message_content_blocks(&second)
        .into_iter()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_result"))
        .collect::<Vec<_>>();
    assert_eq!(tool_results.len(), 2, "second Claude request: {second}");
    assert_eq!(tool_results[0]["tool_use_id"].as_str(), Some("toolu_1"));
    assert!(
        tool_results[0]["content"]
            .as_str()
            .expect("first tool_result content")
            .contains("Paris")
    );
    assert_eq!(tool_results[1]["tool_use_id"].as_str(), Some("toolu_2"));
    assert!(
        tool_results[1]["content"]
            .as_str()
            .expect("second tool_result content")
            .contains("Rome")
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn claude_wire_marks_tool_result_errors() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let responses = mount_claude_sse_sequence(
        &server,
        vec![
            claude_invalid_tool_use_sse(),
            claude_text_sse("msg_2", "handled"),
        ],
    )
    .await;

    let test = test_codex()
        .with_config(configure_claude_provider)
        .build(&server)
        .await?;

    test.submit_turn("run a malformed shell command").await?;

    let requests = responses.requests();
    assert_eq!(requests.len(), 2);

    let second = requests[1].body_json();
    let tool_result = message_content_blocks(&second)
        .into_iter()
        .find(|block| {
            block.get("type").and_then(Value::as_str) == Some("tool_result")
                && block.get("tool_use_id").and_then(Value::as_str) == Some("toolu_bad")
        })
        .unwrap_or_else(|| {
            panic!("second Claude request should include error tool_result: {second}")
        });

    assert_eq!(tool_result["is_error"].as_bool(), Some(true));
    let content = tool_result["content"]
        .as_str()
        .expect("tool_result error content");
    assert!(
        content.contains("missing field `cmd`") || content.contains("missing field"),
        "unexpected error content: {content}"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn claude_wire_exec_command_uses_existing_approval_flow() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let responses = mount_claude_sse_sequence(
        &server,
        vec![
            claude_escalated_exec_tool_use_sse(),
            claude_text_sse("msg_2", "handled denial"),
        ],
    )
    .await;

    let test = test_codex()
        .with_config(configure_claude_provider)
        .build(&server)
        .await?;

    let sandbox_policy = SandboxPolicy::WorkspaceWrite {
        writable_roots: Vec::new(),
        network_access: false,
        exclude_tmpdir_env_var: false,
        exclude_slash_tmp: false,
    };
    let permission_profile = PermissionProfile::from_legacy_sandbox_policy_for_cwd(
        &sandbox_policy,
        test.config.cwd.as_path(),
    );
    test.codex
        .submit(Op::UserTurn {
            environments: None,
            items: vec![UserInput::Text {
                text: "run an escalated Claude shell command".into(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            cwd: test.config.cwd.to_path_buf(),
            approval_policy: AskForApproval::OnRequest,
            approvals_reviewer: None,
            sandbox_policy,
            permission_profile: Some(permission_profile),
            model: test.session_configured.model.clone(),
            effort: None,
            summary: None,
            service_tier: None,
            collaboration_mode: None,
            personality: None,
        })
        .await?;

    let approval = wait_for_event_match(&test.codex, |event| match event {
        EventMsg::ExecApprovalRequest(approval) => Some(approval.clone()),
        _ => None,
    })
    .await;
    assert_eq!(approval.call_id, "toolu_approval");
    assert!(
        approval
            .command
            .iter()
            .any(|part| part.contains("printf approval")),
        "unexpected approval command: {:?}",
        approval.command
    );

    test.codex
        .submit(Op::ExecApproval {
            id: approval.effective_approval_id(),
            turn_id: None,
            decision: ReviewDecision::Denied,
        })
        .await?;

    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    let requests = responses.requests();
    assert_eq!(requests.len(), 2);
    let second = requests[1].body_json();
    let tool_result = message_content_blocks(&second)
        .into_iter()
        .find(|block| {
            block.get("type").and_then(Value::as_str) == Some("tool_result")
                && block.get("tool_use_id").and_then(Value::as_str) == Some("toolu_approval")
        })
        .unwrap_or_else(|| {
            panic!("second Claude request should include denied approval result: {second}")
        });
    assert_eq!(tool_result["is_error"].as_bool(), Some(true));

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn claude_wire_pause_turn_continues_with_assistant_content() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let responses = mount_claude_sse_sequence(
        &server,
        vec![
            claude_pause_turn_sse("msg_1", "working so far"),
            claude_text_sse("msg_2", "done"),
        ],
    )
    .await;

    let test = test_codex()
        .with_config(configure_claude_provider)
        .build(&server)
        .await?;

    test.submit_turn("continue a long Claude turn").await?;

    let requests = responses.requests();
    assert_eq!(requests.len(), 2);

    let second = requests[1].body_json();
    let assistant_text = assistant_message_content_blocks(&second)
        .into_iter()
        .find(|block| {
            block.get("type").and_then(Value::as_str) == Some("text")
                && block.get("text").and_then(Value::as_str) == Some("working so far")
        });
    assert!(
        assistant_text.is_some(),
        "second Claude request should include paused assistant content: {second}"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn claude_wire_round_trips_provider_state_on_pause_turn() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let responses = mount_claude_sse_sequence(
        &server,
        vec![
            claude_pause_turn_with_provider_state_sse(),
            claude_text_sse("msg_2", "done"),
        ],
    )
    .await;

    let test = test_codex()
        .with_config(configure_claude_provider)
        .build(&server)
        .await?;

    test.submit_turn("continue with provider state").await?;

    let requests = responses.requests();
    assert_eq!(requests.len(), 2);

    let second = requests[1].body_json();
    let provider_state = assistant_message_content_blocks(&second)
        .into_iter()
        .find(|block| block.get("type").and_then(Value::as_str) == Some("compaction"))
        .unwrap_or_else(|| {
            panic!("second Claude request should include compaction block: {second}")
        });
    assert_eq!(provider_state["id"].as_str(), Some("compact_1"));
    assert_eq!(provider_state["state"]["cursor"].as_str(), Some("abc"));

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn claude_wire_replays_interleaved_state_and_tool_use_in_order() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let responses = mount_claude_sse_sequence(
        &server,
        vec![
            claude_interleaved_state_tool_use_sse(),
            claude_text_sse("msg_2", "done"),
        ],
    )
    .await;
    mount_claude_count_tokens_response(
        &server,
        ResponseTemplate::new(200).set_body_json(json!({ "input_tokens": 789 })),
        1,
    )
    .await;

    let test = test_codex()
        .with_config(configure_claude_provider)
        .build(&server)
        .await?;

    test.submit_turn("preserve interleaved Claude state")
        .await?;

    let requests = responses.requests();
    assert_eq!(requests.len(), 2);

    let second = requests[1].body_json();
    let messages = second["messages"].as_array().expect("messages array");
    let assistant = messages
        .iter()
        .find(|message| message["role"].as_str() == Some("assistant"))
        .unwrap_or_else(|| {
            panic!("second Claude request should include assistant history: {second}")
        });
    let content = assistant["content"].as_array().expect("assistant content");

    assert_eq!(content.len(), 4, "assistant content: {content:?}");
    assert_eq!(content[0], json!({"type": "text", "text": "intro"}));
    assert_eq!(
        content[1],
        json!({
            "type": "thinking",
            "thinking": "ponder",
            "signature": "sig-a"
        })
    );
    assert_eq!(
        content[2],
        json!({
            "type": "redacted_thinking",
            "data": "opaque-provider-state"
        })
    );
    assert_eq!(
        content[3],
        json!({
            "type": "tool_use",
            "id": "toolu_interleaved",
            "name": "exec_command",
            "input": {"cmd": "printf interleaved"}
        })
    );

    let tool_result = messages
        .iter()
        .find(|message| {
            message["role"].as_str() == Some("user")
                && message["content"].as_array().is_some_and(|content| {
                    content.iter().any(|block| {
                        block.get("type").and_then(Value::as_str) == Some("tool_result")
                            && block.get("tool_use_id").and_then(Value::as_str)
                                == Some("toolu_interleaved")
                    })
                })
        })
        .unwrap_or_else(|| panic!("second Claude request should include tool result: {second}"));
    assert!(
        tool_result["content"][0]["content"]
            .as_str()
            .expect("tool_result content")
            .contains("interleaved")
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn claude_wire_caps_repeated_pause_turn_continuations() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let responses = mount_claude_sse_sequence(
        &server,
        vec![
            claude_pause_turn_sse("msg_1", "pause 1"),
            claude_pause_turn_sse("msg_2", "pause 2"),
            claude_pause_turn_sse("msg_3", "pause 3"),
            claude_pause_turn_sse("msg_4", "pause 4"),
        ],
    )
    .await;

    let test = test_codex()
        .with_config(configure_claude_provider)
        .build(&server)
        .await?;

    test.codex
        .submit(Op::UserInput {
            environments: None,
            items: vec![UserInput::Text {
                text: "trigger repeated pause_turn".into(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
        })
        .await?;

    let error = wait_for_event_match(&test.codex, |event| match event {
        EventMsg::Error(error) => Some(error.message.clone()),
        _ => None,
    })
    .await;
    assert!(
        error.contains("pause_turn"),
        "expected pause_turn cap error, got: {error}"
    );

    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;
    assert_eq!(responses.requests().len(), 4);

    Ok(())
}

fn configure_claude_provider(config: &mut Config) {
    config.model_provider.name = "Anthropic".to_string();
    config.model_provider.env_key = Some(EXISTING_ENV_VAR_WITH_NON_EMPTY_VALUE.to_string());
    config.model_provider.experimental_bearer_token = None;
    config.model_provider.requires_openai_auth = false;
    config.model_provider.supports_websockets = false;
    config.model_provider.wire_api = WireApi::Claude;
}

fn claude_pause_turn_sse(message_id: &str, text: &str) -> String {
    sse(vec![
        json!({
            "type": "message_start",
            "message": {
                "id": message_id,
                "type": "message",
                "role": "assistant",
                "content": [],
                "usage": {"input_tokens": 1, "output_tokens": 1}
            }
        }),
        json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": {"type": "text", "text": ""}
        }),
        json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": {"type": "text_delta", "text": text}
        }),
        json!({"type": "content_block_stop", "index": 0}),
        json!({
            "type": "message_delta",
            "delta": {"stop_reason": "pause_turn"},
            "usage": {"output_tokens": 5}
        }),
        claude_message_stop(),
    ])
}

fn claude_pause_turn_with_provider_state_sse() -> String {
    sse(vec![
        json!({
            "type": "message_start",
            "message": {
                "id": "msg_1",
                "type": "message",
                "role": "assistant",
                "content": [],
                "usage": {"input_tokens": 1, "output_tokens": 1}
            }
        }),
        json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": {"type": "text", "text": ""}
        }),
        json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": {"type": "text_delta", "text": "working with state"}
        }),
        json!({"type": "content_block_stop", "index": 0}),
        json!({
            "type": "content_block_start",
            "index": 1,
            "content_block": {
                "type": "compaction",
                "id": "compact_1",
                "state": {"cursor": "abc"}
            }
        }),
        json!({"type": "content_block_stop", "index": 1}),
        json!({
            "type": "message_delta",
            "delta": {"stop_reason": "pause_turn"},
            "usage": {"output_tokens": 5}
        }),
        claude_message_stop(),
    ])
}

fn claude_interleaved_state_tool_use_sse() -> String {
    sse(vec![
        json!({
            "type": "message_start",
            "message": {
                "id": "msg_1",
                "type": "message",
                "role": "assistant",
                "content": [],
                "usage": {"input_tokens": 1, "output_tokens": 1}
            }
        }),
        json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": {"type": "text", "text": "intro"}
        }),
        json!({"type": "content_block_stop", "index": 0}),
        json!({
            "type": "content_block_start",
            "index": 1,
            "content_block": {"type": "thinking", "thinking": "ponder"}
        }),
        json!({
            "type": "content_block_delta",
            "index": 1,
            "delta": {"type": "signature_delta", "signature": "sig-a"}
        }),
        json!({"type": "content_block_stop", "index": 1}),
        json!({
            "type": "content_block_start",
            "index": 2,
            "content_block": {
                "type": "redacted_thinking",
                "data": "opaque-provider-state"
            }
        }),
        json!({"type": "content_block_stop", "index": 2}),
        json!({
            "type": "content_block_start",
            "index": 3,
            "content_block": {
                "type": "tool_use",
                "id": "toolu_interleaved",
                "name": "exec_command",
                "input": {}
            }
        }),
        json!({
            "type": "content_block_delta",
            "index": 3,
            "delta": {
                "type": "input_json_delta",
                "partial_json": "{\"cmd\":\"printf interleaved\"}"
            }
        }),
        json!({"type": "content_block_stop", "index": 3}),
        json!({
            "type": "message_delta",
            "delta": {"stop_reason": "tool_use"},
            "usage": {"output_tokens": 5}
        }),
        claude_message_stop(),
    ])
}

fn claude_tool_use_sse() -> String {
    sse(vec![
        json!({
            "type": "message_start",
            "message": {
                "id": "msg_1",
                "type": "message",
                "role": "assistant",
                "content": [],
                "usage": {"input_tokens": 1, "output_tokens": 1}
            }
        }),
        json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": {
                "type": "tool_use",
                "id": "toolu_1",
                "name": "exec_command",
                "input": {}
            }
        }),
        json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": {
                "type": "input_json_delta",
                "partial_json": "{\"cmd\":\"printf claude\"}"
            }
        }),
        json!({"type": "content_block_stop", "index": 0}),
        json!({
            "type": "message_delta",
            "delta": {"stop_reason": "tool_use"},
            "usage": {"output_tokens": 5}
        }),
        claude_message_stop(),
    ])
}

fn claude_invalid_tool_use_sse() -> String {
    sse(vec![
        json!({
            "type": "message_start",
            "message": {
                "id": "msg_1",
                "type": "message",
                "role": "assistant",
                "content": [],
                "usage": {"input_tokens": 1, "output_tokens": 1}
            }
        }),
        json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": {
                "type": "tool_use",
                "id": "toolu_bad",
                "name": "exec_command",
                "input": {}
            }
        }),
        json!({
            "type": "message_delta",
            "delta": {"stop_reason": "tool_use"},
            "usage": {"output_tokens": 5}
        }),
        claude_message_stop(),
    ])
}

fn claude_escalated_exec_tool_use_sse() -> String {
    sse(vec![
        json!({
            "type": "message_start",
            "message": {
                "id": "msg_1",
                "type": "message",
                "role": "assistant",
                "content": [],
                "usage": {"input_tokens": 1, "output_tokens": 1}
            }
        }),
        json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": {
                "type": "tool_use",
                "id": "toolu_approval",
                "name": "exec_command",
                "input": {}
            }
        }),
        json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": {
                "type": "input_json_delta",
                "partial_json": "{\"cmd\":\"printf approval\",\"sandbox_permissions\":\"require_escalated\",\"justification\":\"Need full access for approval test\"}"
            }
        }),
        json!({"type": "content_block_stop", "index": 0}),
        json!({
            "type": "message_delta",
            "delta": {"stop_reason": "tool_use"},
            "usage": {"output_tokens": 5}
        }),
        claude_message_stop(),
    ])
}

fn claude_multi_tool_use_sse() -> String {
    sse(vec![
        json!({
            "type": "message_start",
            "message": {
                "id": "msg_1",
                "type": "message",
                "role": "assistant",
                "content": [],
                "usage": {"input_tokens": 1, "output_tokens": 1}
            }
        }),
        json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": {
                "type": "tool_use",
                "id": "toolu_1",
                "name": "exec_command",
                "input": {}
            }
        }),
        json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": {
                "type": "input_json_delta",
                "partial_json": "{\"cmd\":\"printf Paris\"}"
            }
        }),
        json!({"type": "content_block_stop", "index": 0}),
        json!({
            "type": "content_block_start",
            "index": 1,
            "content_block": {
                "type": "tool_use",
                "id": "toolu_2",
                "name": "exec_command",
                "input": {}
            }
        }),
        json!({
            "type": "content_block_delta",
            "index": 1,
            "delta": {
                "type": "input_json_delta",
                "partial_json": "{\"cmd\":\"printf "
            }
        }),
        json!({
            "type": "content_block_delta",
            "index": 1,
            "delta": {
                "type": "input_json_delta",
                "partial_json": "Rome\"}"
            }
        }),
        json!({"type": "content_block_stop", "index": 1}),
        json!({
            "type": "message_delta",
            "delta": {"stop_reason": "tool_use"},
            "usage": {"output_tokens": 5}
        }),
        claude_message_stop(),
    ])
}

fn claude_text_sse(message_id: &str, text: &str) -> String {
    sse(vec![
        json!({
            "type": "message_start",
            "message": {
                "id": message_id,
                "type": "message",
                "role": "assistant",
                "content": [],
                "usage": {"input_tokens": 1, "output_tokens": 1}
            }
        }),
        json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": {"type": "text", "text": ""}
        }),
        json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": {"type": "text_delta", "text": text}
        }),
        json!({"type": "content_block_stop", "index": 0}),
        claude_message_stop(),
    ])
}

fn claude_text_sse_without_usage(message_id: &str, text: &str) -> String {
    sse(vec![
        json!({
            "type": "message_start",
            "message": {
                "id": message_id,
                "type": "message",
                "role": "assistant",
                "content": []
            }
        }),
        json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": {"type": "text", "text": ""}
        }),
        json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": {"type": "text_delta", "text": text}
        }),
        json!({"type": "content_block_stop", "index": 0}),
        json!({
            "type": "message_delta",
            "delta": {"stop_reason": "end_turn"},
            "usage": {"output_tokens": 0}
        }),
        claude_message_stop(),
    ])
}

fn claude_message_stop() -> Value {
    json!({
        "type": "message_stop",
        "message": null
    })
}

fn tool_names(body: &Value) -> Vec<String> {
    let Some(tools) = body["tools"].as_array() else {
        panic!("tools array");
    };
    tools
        .iter()
        .filter_map(|tool| tool["name"].as_str().map(str::to_string))
        .collect()
}

fn message_content_blocks(body: &Value) -> Vec<Value> {
    let Some(messages) = body["messages"].as_array() else {
        panic!("messages array");
    };
    messages
        .iter()
        .filter_map(|message| message["content"].as_array())
        .flatten()
        .cloned()
        .collect()
}

fn assistant_message_content_blocks(body: &Value) -> Vec<Value> {
    let Some(messages) = body["messages"].as_array() else {
        panic!("messages array");
    };
    messages
        .iter()
        .filter(|message| message["role"].as_str() == Some("assistant"))
        .filter_map(|message| message["content"].as_array())
        .flatten()
        .cloned()
        .collect()
}
