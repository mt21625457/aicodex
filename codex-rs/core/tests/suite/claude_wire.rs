use std::time::Duration;

use codex_core::compact::SUMMARIZATION_PROMPT;
use codex_core::compact::SUMMARY_PREFIX;
use codex_core::config::Config;
use codex_features::Feature;
use codex_model_provider_info::WireApi;
use codex_protocol::config_types::AutoCompactTokenLimitScope;
use codex_protocol::config_types::WebSearchConfig;
use codex_protocol::config_types::WebSearchContextSize;
use codex_protocol::config_types::WebSearchMode;
use codex_protocol::items::TurnItem;
use codex_protocol::models::PermissionProfile;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::CodexErrorInfo;
use codex_protocol::protocol::ContextTokenUsageSource;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::FileChange;
use codex_protocol::protocol::Op;
use codex_protocol::protocol::ReviewDecision;
use codex_protocol::protocol::SandboxPolicy;
use codex_protocol::protocol::TokenUsageInfo;
use codex_protocol::user_input::UserInput;
use core_test_support::responses::ev_assistant_message;
use core_test_support::responses::ev_completed_with_tokens;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::mount_claude_count_tokens_never;
use core_test_support::responses::mount_claude_count_tokens_response;
use core_test_support::responses::mount_claude_sse_sequence;
use core_test_support::responses::mount_claude_sse_sequence_with_delays;
use core_test_support::responses::mount_sse_sequence;
use core_test_support::responses::sse;
use core_test_support::responses::start_mock_server;
use core_test_support::test_codex::TestCodex;
use core_test_support::test_codex::local;
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
            environments: Some(vec![local(test.config.cwd.clone())]),
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

async fn submit_user_input_without_waiting(test: &TestCodex, prompt: &str) -> anyhow::Result<()> {
    test.codex
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: prompt.into(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
            additional_context: Default::default(),
            thread_settings: Default::default(),
        })
        .await?;
    Ok(())
}

async fn collect_error_infos_until_turn_complete(test: &TestCodex) -> Vec<Option<CodexErrorInfo>> {
    collect_events_until_turn_complete(test)
        .await
        .into_iter()
        .filter_map(|event| match event {
            EventMsg::Error(error) => Some(error.codex_error_info),
            _ => None,
        })
        .collect()
}

async fn collect_events_until_turn_complete(test: &TestCodex) -> Vec<EventMsg> {
    let mut events = Vec::new();
    loop {
        let event = wait_for_event(&test.codex, |_| true).await;
        let turn_complete = matches!(event, EventMsg::TurnComplete(_));
        events.push(event);
        if turn_complete {
            return events;
        }
    }
}

fn count_context_window_errors(events: &[EventMsg]) -> usize {
    events
        .iter()
        .filter(|event| {
            matches!(
                event,
                EventMsg::Error(error)
                    if error.codex_error_info.as_ref() == Some(&CodexErrorInfo::ContextWindowExceeded)
            )
        })
        .count()
}

async fn count_context_window_errors_until_turn_complete(test: &TestCodex) -> usize {
    count_context_window_errors(&collect_events_until_turn_complete(test).await)
}

fn token_usage_with_source(events: &[EventMsg], source: ContextTokenUsageSource) -> TokenUsageInfo {
    events
        .iter()
        .find_map(|event| match event {
            EventMsg::TokenCount(payload)
                if payload
                    .info
                    .as_ref()
                    .is_some_and(|info| info.context_source == Some(source)) =>
            {
                payload.info.clone()
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("expected TokenCount with context source {source:?}"))
}

fn compact_prompt_head() -> &'static str {
    SUMMARIZATION_PROMPT
        .lines()
        .next()
        .unwrap_or(SUMMARIZATION_PROMPT)
}

fn claude_request_text(body: &Value) -> String {
    message_content_blocks(body)
        .into_iter()
        .filter_map(|block| {
            block
                .get("text")
                .or_else(|| block.get("content"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn assert_claude_request_contains(body: &Value, expected: &str, label: &str) {
    let text = claude_request_text(body);
    assert!(
        text.contains(expected),
        "{label} should contain {expected:?}; request text was:\n{text}"
    );
}

fn assert_claude_request_excludes(body: &Value, unexpected: &str, label: &str) {
    let text = claude_request_text(body);
    assert!(
        !text.contains(unexpected),
        "{label} should not contain {unexpected:?}; request text was:\n{text}"
    );
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
        .with_model("gpt-5.2")
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
    assert!(
        first["tools"]
            .as_array()
            .is_some_and(|tools| tools
                .iter()
                .any(
                    |tool| tool.get("name").and_then(Value::as_str) == Some("web_search")
                        && tool.get("type").and_then(Value::as_str) == Some("web_search_20250305")
                        && tool.get("input_schema").is_none()
                )),
        "Claude requests should advertise native web_search server tool: {first}"
    );
    assert!(
        !first_tool_names
            .iter()
            .any(|name| name == "image_generation"),
        "Claude requests must not advertise hosted image_generation: {first_tool_names:?}"
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
    let counted_tool_result = tool_result_block(&count_request, "toolu_1");
    assert!(
        counted_tool_result["content"]
            .as_str()
            .expect("tool_result content")
            .contains("claude"),
        "count_tokens request should include the model-visible tool result: {count_request}"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn claude_wire_local_web_search_fallback_posts_function_tool_result() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let responses = mount_claude_sse_sequence(
        &server,
        vec![
            claude_custom_tool_use_sse("toolu_web_search", "web_search", json!({})),
            claude_text_sse("msg_2", "handled"),
        ],
    )
    .await;

    let test = test_codex()
        .with_model("gpt-5.2")
        .with_config(configure_claude_provider_with_web_search_context_size)
        .build(&server)
        .await?;

    test.submit_turn("search through local fallback").await?;

    let requests = responses.requests();
    assert_eq!(requests.len(), 2);
    let first = requests[0].body_json();
    let web_search_tool = first["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .find(|tool| tool.get("name").and_then(Value::as_str) == Some("web_search"))
        .unwrap_or_else(|| panic!("first Claude request should include web_search: {first}"));
    assert!(web_search_tool.get("type").is_none());
    assert_eq!(
        web_search_tool
            .get("input_schema")
            .and_then(|schema| schema.get("properties"))
            .and_then(|properties| properties.get("query"))
            .and_then(|query| query.get("type"))
            .and_then(Value::as_str),
        Some("string")
    );

    let second = requests[1].body_json();
    let tool_result = tool_result_block(&second, "toolu_web_search");
    assert_eq!(tool_result["is_error"].as_bool(), Some(true));
    let content = tool_result["content"]
        .as_str()
        .expect("tool_result error content");
    assert!(
        content.contains("web_search requires `query` or `queries`"),
        "unexpected error content: {content}"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn claude_wire_stream_close_after_tool_call_continues_with_tool_follow_up()
-> anyhow::Result<()> {
    let server = start_mock_server().await;
    let incomplete_tool_use = sse(vec![
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
    ]);
    let responses = mount_claude_sse_sequence(
        &server,
        vec![incomplete_tool_use, claude_text_sse("msg_2", "done")],
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
    let second = requests[1].body_json();
    let tool_result = tool_result_block(&second, "toolu_1");
    assert_eq!(tool_result["tool_use_id"].as_str(), Some("toolu_1"));
    assert!(
        tool_result["content"]
            .as_str()
            .expect("tool_result content")
            .contains("claude")
    );
    assert_eq!(count_tokens.requests().len(), 1);

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
                        info.context_tokens == Some(456)
                            && info.context_source
                                == Some(ContextTokenUsageSource::ClaudeCountTokens)
                            && info.last_token_usage.total_tokens == 456
                            && info.last_token_usage.input_tokens == 456
                    })
            )
        })
        .await?;
    let EventMsg::TokenCount(payload) = token_event else {
        unreachable!("wait_for_event returned unexpected event");
    };
    let info = payload.info.expect("token usage info");
    assert_eq!(info.context_tokens, Some(456));
    assert_eq!(
        info.context_source,
        Some(ContextTokenUsageSource::ClaudeCountTokens)
    );
    assert_eq!(info.last_token_usage.total_tokens, 456);

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
async fn claude_wire_emits_in_flight_estimate_before_final_context_usage() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    mount_claude_sse_sequence(
        &server,
        vec![claude_text_sse_without_usage("msg_1", "done")],
    )
    .await;
    mount_claude_count_tokens_response(
        &server,
        ResponseTemplate::new(200).set_body_json(json!({ "input_tokens": 456 })),
        1,
    )
    .await;

    let test = test_codex()
        .with_config(configure_claude_provider)
        .build(&server)
        .await?;

    test.codex
        .submit(Op::UserTurn {
            environments: Some(vec![local(test.config.cwd.clone())]),
            items: vec![UserInput::Text {
                text: "show a running Claude context estimate".into(),
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

    let in_flight = wait_for_event_match(&test.codex, |event| match event {
        EventMsg::TokenCount(payload)
            if payload.info.as_ref().is_some_and(|info| {
                info.context_source == Some(ContextTokenUsageSource::InFlightEstimate)
            }) =>
        {
            payload.info.clone()
        }
        _ => None,
    })
    .await;
    assert_eq!(
        in_flight.context_source,
        Some(ContextTokenUsageSource::InFlightEstimate)
    );
    assert!(
        in_flight.context_tokens.is_some_and(|tokens| tokens > 0),
        "in-flight estimate should include a positive context count: {in_flight:?}"
    );

    let final_info = wait_for_event_match(&test.codex, |event| match event {
        EventMsg::TokenCount(payload)
            if payload.info.as_ref().is_some_and(|info| {
                info.context_source == Some(ContextTokenUsageSource::ClaudeCountTokens)
            }) =>
        {
            payload.info.clone()
        }
        _ => None,
    })
    .await;
    assert_eq!(final_info.context_tokens, Some(456));
    assert_eq!(
        final_info.context_source,
        Some(ContextTokenUsageSource::ClaudeCountTokens)
    );

    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn compatible_count_tokens_success_updates_context_usage() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let responses = mount_claude_sse_sequence(
        &server,
        vec![claude_text_sse_with_usage("msg_1", "done", 333, 12)],
    )
    .await;
    let count_tokens = mount_claude_count_tokens_response(
        &server,
        ResponseTemplate::new(200).set_body_json(json!({ "input_tokens": 777 })),
        1,
    )
    .await;

    let test = test_codex()
        .with_config(configure_compatible_claude_provider)
        .build(&server)
        .await?;

    let token_event = submit_turn_and_wait_for_token_event(
        &test,
        "count this compatible Claude context",
        |event| {
            matches!(
                event,
                EventMsg::TokenCount(payload)
                    if payload.info.as_ref().is_some_and(|info| {
                        info.context_tokens == Some(777)
                            && info.context_source
                                == Some(ContextTokenUsageSource::ClaudeCountTokens)
                            && info.last_token_usage.total_tokens == 777
                    })
            )
        },
    )
    .await?;
    let EventMsg::TokenCount(payload) = token_event else {
        unreachable!("wait_for_event returned unexpected event");
    };
    let info = payload.info.expect("token usage info");
    assert_eq!(info.context_tokens, Some(777));
    assert_eq!(
        info.context_source,
        Some(ContextTokenUsageSource::ClaudeCountTokens)
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
async fn compatible_count_tokens_error_uses_stream_usage() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let responses = mount_claude_sse_sequence(
        &server,
        vec![claude_text_sse_with_usage("msg_1", "done", 333, 12)],
    )
    .await;
    let count_tokens = mount_claude_count_tokens_response(
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
        .with_config(configure_compatible_claude_provider)
        .build(&server)
        .await?;

    let token_event = submit_turn_and_wait_for_token_event(
        &test,
        "count this compatible Claude context",
        |event| {
            matches!(
                event,
                EventMsg::TokenCount(payload)
                    if payload.info.as_ref().is_some_and(|info| {
                        info.context_tokens == Some(345)
                            && info.context_source
                                == Some(ContextTokenUsageSource::ProviderUsage)
                            && info.last_token_usage.total_tokens == 345
                    })
            )
        },
    )
    .await?;
    let EventMsg::TokenCount(payload) = token_event else {
        unreachable!("wait_for_event returned unexpected event");
    };
    let info = payload.info.expect("token usage info");
    assert_eq!(info.context_tokens, Some(345));
    assert_eq!(
        info.context_source,
        Some(ContextTokenUsageSource::ProviderUsage)
    );

    assert_eq!(responses.requests().len(), 1);
    assert_eq!(count_tokens.requests().len(), 1);

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
                        info.context_source == Some(ContextTokenUsageSource::LocalEstimate)
                            && info.context_tokens.is_some_and(|tokens| tokens > 0)
                            && info.last_token_usage.total_tokens > 0
                    })
            )
        },
    )
    .await?;
    let EventMsg::TokenCount(payload) = token_event else {
        unreachable!("wait_for_event returned unexpected event");
    };
    let info = payload.info.expect("token usage info");
    assert!(info.context_tokens.is_some_and(|tokens| tokens > 0));
    assert!(info.last_token_usage.total_tokens > 0);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn claude_wire_zero_count_tokens_uses_local_context_estimate() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    mount_claude_sse_sequence(
        &server,
        vec![claude_text_sse_without_usage("msg_1", "done")],
    )
    .await;
    mount_claude_count_tokens_response(
        &server,
        ResponseTemplate::new(200).set_body_json(json!({ "input_tokens": 0 })),
        1,
    )
    .await;

    let test = test_codex()
        .with_config(configure_claude_provider)
        .build(&server)
        .await?;

    let token_event = submit_turn_and_wait_for_token_event(
        &test,
        "count this Claude context with zero native count",
        |event| {
            matches!(
                event,
                EventMsg::TokenCount(payload)
                    if payload.info.as_ref().is_some_and(|info| {
                        info.context_source == Some(ContextTokenUsageSource::LocalEstimate)
                            && info.context_tokens.is_some_and(|tokens| tokens > 0)
                            && info.last_token_usage.total_tokens > 0
                    })
            )
        },
    )
    .await?;
    let EventMsg::TokenCount(payload) = token_event else {
        unreachable!("wait_for_event returned unexpected event");
    };
    let info = payload.info.expect("token usage info");
    assert_eq!(
        info.context_source,
        Some(ContextTokenUsageSource::LocalEstimate)
    );
    assert!(info.context_tokens.is_some_and(|tokens| tokens > 0));
    assert!(info.last_token_usage.total_tokens > 0);

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
                        info.total_token_usage.total_tokens == 321
                            && info.context_source == Some(ContextTokenUsageSource::LocalEstimate)
                            && info.context_tokens.is_some_and(|tokens| tokens >= 321)
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
            .total_token_usage
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
async fn claude_wire_marks_malformed_custom_tool_input_errors() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let responses = mount_claude_sse_sequence(
        &server,
        vec![
            claude_custom_tool_use_sse("toolu_bad_custom", "apply_patch", json!({})),
            claude_text_sse("msg_2", "handled"),
        ],
    )
    .await;

    let test = test_codex()
        .with_config(configure_claude_provider_with_apply_patch)
        .build(&server)
        .await?;

    test.submit_turn("call malformed apply_patch").await?;

    let requests = responses.requests();
    assert_eq!(requests.len(), 2);
    let second = requests[1].body_json();
    let tool_result = tool_result_block(&second, "toolu_bad_custom");
    assert_eq!(tool_result["is_error"].as_bool(), Some(true));
    let content = tool_result["content"]
        .as_str()
        .expect("tool_result error content");
    assert!(
        content.contains("must include an `input` string"),
        "unexpected error content: {content}"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn claude_wire_marks_malformed_code_mode_exec_input_errors() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let responses = mount_claude_sse_sequence(
        &server,
        vec![
            claude_custom_tool_use_sse("toolu_bad_exec", "exec", json!({})),
            claude_text_sse("msg_2", "handled"),
        ],
    )
    .await;

    let test = test_codex()
        .with_config(configure_claude_provider_with_code_mode)
        .build(&server)
        .await?;

    test.submit_turn("call malformed code mode exec").await?;

    let requests = responses.requests();
    assert_eq!(requests.len(), 2);

    let first = requests[0].body_json();
    let first_tool_names = tool_names(&first);
    assert!(
        first_tool_names.iter().any(|name| name == "exec"),
        "Code Mode Claude request should advertise exec: {first_tool_names:?}"
    );

    let second = requests[1].body_json();
    let tool_result = tool_result_block(&second, "toolu_bad_exec");
    assert_eq!(tool_result["is_error"].as_bool(), Some(true));
    let content = tool_result["content"]
        .as_str()
        .expect("tool_result error content");
    assert!(
        content.contains("must include an `input` string"),
        "unexpected error content: {content}"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn claude_wire_reports_invalid_apply_patch_header() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let responses = mount_claude_sse_sequence(
        &server,
        vec![
            claude_custom_tool_use_sse(
                "toolu_bad_patch",
                "apply_patch",
                json!({"input": "Create file: report.md\n# Report\n"}),
            ),
            claude_text_sse("msg_2", "handled"),
        ],
    )
    .await;

    let test = test_codex()
        .with_config(configure_claude_provider_with_apply_patch)
        .build(&server)
        .await?;

    test.submit_turn("call apply_patch with a bad header")
        .await?;

    let requests = responses.requests();
    assert_eq!(requests.len(), 2);
    let second = requests[1].body_json();
    let tool_result = tool_result_block(&second, "toolu_bad_patch");
    assert_eq!(tool_result["is_error"].as_bool(), Some(true));
    let content = tool_result["content"]
        .as_str()
        .expect("tool_result error content");
    assert!(
        content.contains("*** Begin Patch") || content.contains("first line"),
        "unexpected error content: {content}"
    );
    assert!(!test.config.cwd.join("report.md").exists());

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn claude_wire_allows_corrected_apply_patch_after_missing_add_prefix() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let responses = mount_claude_sse_sequence(
        &server,
        vec![
            claude_custom_tool_use_sse(
                "toolu_bad_add",
                "apply_patch",
                json!({
                    "input": "*** Begin Patch\n*** Add File: claude_retry.txt\nplain text\n*** End Patch"
                }),
            ),
            claude_custom_tool_use_sse(
                "toolu_good_add",
                "apply_patch",
                json!({
                    "input": "*** Begin Patch\n*** Add File: claude_retry.txt\n+plain text\n*** End Patch"
                }),
            ),
            claude_text_sse("msg_3", "done"),
        ],
    )
    .await;

    let test = test_codex()
        .with_config(configure_claude_provider_with_apply_patch)
        .build(&server)
        .await?;

    test.submit_turn("retry apply_patch with valid plus prefixes")
        .await?;

    let requests = responses.requests();
    assert_eq!(requests.len(), 3);

    let second = requests[1].body_json();
    let bad_result = tool_result_block(&second, "toolu_bad_add");
    assert_eq!(bad_result["is_error"].as_bool(), Some(true));
    let bad_content = bad_result["content"]
        .as_str()
        .expect("bad tool_result content");
    assert!(
        bad_content.contains("Add file content line must start with '+'"),
        "unexpected bad apply_patch content: {bad_content}"
    );

    let third = requests[2].body_json();
    let good_result = tool_result_block(&third, "toolu_good_add");
    assert_ne!(good_result["is_error"].as_bool(), Some(true));
    let written = std::fs::read_to_string(test.config.cwd.join("claude_retry.txt"))?;
    assert_eq!(written, "plain text\n");

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn claude_wire_apply_patch_wrapper_streams_raw_patch_update() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let patch = "*** Begin Patch\n*** Add File: claude_streamed.txt\n+hello\n*** End Patch";
    let partial_json = serde_json::to_string(&json!({ "input": patch }))?;
    let responses = mount_claude_sse_sequence(
        &server,
        vec![
            claude_custom_tool_use_delta_sse("toolu_stream", "apply_patch", &partial_json),
            claude_text_sse("msg_2", "done"),
        ],
    )
    .await;

    let test = test_codex()
        .with_config(configure_claude_provider_with_apply_patch_streaming)
        .build(&server)
        .await?;

    test.codex
        .submit(Op::UserTurn {
            environments: Some(vec![local(test.config.cwd.clone())]),
            items: vec![UserInput::Text {
                text: "stream an apply_patch tool call".into(),
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

    let mut updates = Vec::new();
    wait_for_event(&test.codex, |event| match event {
        EventMsg::PatchApplyUpdated(update) => {
            updates.push(update.clone());
            false
        }
        EventMsg::TurnComplete(_) => true,
        _ => false,
    })
    .await;

    assert_eq!(responses.requests().len(), 2);
    assert!(
        updates
            .iter()
            .any(|update| update.call_id == "toolu_stream"),
        "Claude apply_patch stream should emit raw patch updates: {updates:?}"
    );
    let latest_change = updates
        .iter()
        .rev()
        .find_map(|update| {
            update
                .changes
                .get(&std::path::PathBuf::from("claude_streamed.txt"))
        })
        .expect("streamed apply_patch update for claude_streamed.txt");
    assert_eq!(
        latest_change,
        &FileChange::Add {
            content: "hello\n".to_string(),
        }
    );
    let written = std::fs::read_to_string(test.config.cwd.join("claude_streamed.txt"))?;
    assert_eq!(written, "hello\n");

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn claude_wire_stream_close_after_tool_input_does_not_retry() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let patch = "*** Begin Patch\n*** Add File: claude_partial.txt\n+hello\n*** End Patch";
    let partial_json = serde_json::to_string(&json!({ "input": patch }))?;
    let incomplete_tool_input = sse(vec![
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
                "id": "toolu_stream",
                "name": "apply_patch",
                "input": {}
            }
        }),
        json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": {
                "type": "input_json_delta",
                "partial_json": partial_json
            }
        }),
    ]);
    let responses = mount_claude_sse_sequence(&server, vec![incomplete_tool_input]).await;
    mount_claude_count_tokens_never(&server).await;

    let test = test_codex()
        .with_config(configure_claude_provider_with_apply_patch_streaming)
        .build(&server)
        .await?;

    test.submit_turn("stream an apply_patch tool call").await?;

    assert_eq!(responses.requests().len(), 1);
    assert!(
        !test.config.cwd.join("claude_partial.txt").exists(),
        "partial streamed tool input must not execute after stream close"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn compatible_claude_wire_exposes_apply_patch_for_fallback_model() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let responses = mount_claude_sse_sequence(
        &server,
        vec![
            claude_custom_tool_use_sse(
                "toolu_compatible_patch",
                "apply_patch",
                json!({
                    "input": "*** Begin Patch\n*** Add File: compatible_patch.txt\n+hello from compatible claude\n*** End Patch"
                }),
            ),
            claude_text_sse("msg_2", "done"),
        ],
    )
    .await;

    let test = test_codex()
        .with_config(configure_compatible_claude_provider_with_fallback_model)
        .build(&server)
        .await?;

    test.submit_turn("edit a file through compatible Claude")
        .await?;

    let requests = responses.requests();
    assert_eq!(requests.len(), 2);

    let first = requests[0].body_json();
    assert_eq!(first["model"].as_str(), Some("deepseek-v4-pro"));
    let first_tool_names = tool_names(&first);
    assert!(
        first_tool_names.iter().any(|name| name == "apply_patch"),
        "compatible Claude request should advertise apply_patch for fallback model metadata: {first_tool_names:?}"
    );
    assert!(
        !first["tools"]
            .as_array()
            .expect("tools array")
            .iter()
            .any(|tool| {
                tool.get("type")
                    .and_then(Value::as_str)
                    .is_some_and(|tool_type| {
                        tool_type.starts_with("text_editor_") || tool_type == "bash_20250124"
                    })
            }),
        "compatible Claude request must stay on fallback tools: {first}"
    );
    let apply_patch_tool = first["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .find(|tool| tool.get("name").and_then(Value::as_str) == Some("apply_patch"))
        .unwrap_or_else(|| panic!("compatible Claude request should include apply_patch: {first}"));
    assert_eq!(
        apply_patch_tool["input_schema"]["required"],
        json!(["input"])
    );

    let second = requests[1].body_json();
    let tool_result = tool_result_block(&second, "toolu_compatible_patch");
    assert_ne!(tool_result["is_error"].as_bool(), Some(true));
    let written = std::fs::read_to_string(test.config.cwd.join("compatible_patch.txt"))?;
    assert_eq!(written, "hello from compatible claude\n");

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn claude_wire_exposes_and_executes_native_text_editor() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let responses = mount_claude_sse_sequence(
        &server,
        vec![
            claude_custom_tool_use_sse(
                "toolu_native_edit",
                "str_replace_based_edit_tool",
                json!({
                    "command": "create",
                    "path": "native_text_editor.txt",
                    "file_text": "hello from native text editor\n"
                }),
            ),
            claude_text_sse("msg_2", "done"),
        ],
    )
    .await;

    let test = test_codex()
        .with_config(configure_claude_provider_with_apply_patch)
        .build(&server)
        .await?;

    test.submit_turn("edit through Claude native text editor")
        .await?;

    let requests = responses.requests();
    assert_eq!(requests.len(), 2);
    let first = requests[0].body_json();
    assert!(
        first["tools"]
            .as_array()
            .expect("tools array")
            .iter()
            .any(|tool| {
                tool.get("name").and_then(Value::as_str) == Some("str_replace_based_edit_tool")
                    && tool
                        .get("type")
                        .and_then(Value::as_str)
                        .is_some_and(|tool_type| tool_type.starts_with("text_editor_"))
            }),
        "Claude request should advertise native text editor: {first}"
    );

    let second = requests[1].body_json();
    let tool_result = tool_result_block(&second, "toolu_native_edit");
    assert_ne!(tool_result["is_error"].as_bool(), Some(true));
    let written = std::fs::read_to_string(test.config.cwd.join("native_text_editor.txt"))?;
    assert_eq!(written, "hello from native text editor\n");

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn claude_wire_native_text_editor_rejects_outside_workspace() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let outside = tempfile::tempdir()?;
    let outside_file = outside.path().join("outside.txt");
    let responses = mount_claude_sse_sequence(
        &server,
        vec![
            claude_custom_tool_use_sse(
                "toolu_native_edit_outside",
                "str_replace_based_edit_tool",
                json!({
                    "command": "create",
                    "path": outside_file.to_string_lossy(),
                    "file_text": "should not write\n"
                }),
            ),
            claude_text_sse("msg_2", "handled"),
        ],
    )
    .await;

    let test = test_codex()
        .with_config(configure_claude_provider_with_apply_patch)
        .build(&server)
        .await?;

    test.submit_turn("try an unsafe native text editor path")
        .await?;

    let second = responses.requests()[1].body_json();
    let tool_result = tool_result_block(&second, "toolu_native_edit_outside");
    assert_eq!(tool_result["is_error"].as_bool(), Some(true));
    assert!(!outside_file.exists());

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn claude_wire_native_bash_uses_existing_shell_executor() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let responses = mount_claude_sse_sequence(
        &server,
        vec![
            claude_custom_tool_use_sse(
                "toolu_native_bash",
                "bash",
                json!({"command": "printf native-bash"}),
            ),
            claude_text_sse("msg_2", "done"),
        ],
    )
    .await;

    let test = test_codex()
        .with_config(configure_claude_provider)
        .build(&server)
        .await?;

    test.submit_turn("run Claude native bash").await?;

    let second = responses.requests()[1].body_json();
    let tool_result = tool_result_block(&second, "toolu_native_bash");
    assert!(
        tool_result["content"]
            .as_str()
            .expect("tool_result content")
            .contains("native-bash")
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn claude_wire_native_bash_uses_existing_approval_denial_flow() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let responses = mount_claude_sse_sequence(
        &server,
        vec![
            claude_custom_tool_use_sse(
                "toolu_native_bash_approval",
                "bash",
                json!({
                    "command": "printf native-approval",
                    "sandbox_permissions": "require_escalated",
                    "justification": "Need full access for native bash approval test"
                }),
            ),
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
            environments: Some(vec![local(test.config.cwd.clone())]),
            items: vec![UserInput::Text {
                text: "run an escalated Claude native bash command".into(),
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
    assert_eq!(approval.call_id, "toolu_native_bash_approval");
    assert!(
        approval
            .command
            .iter()
            .any(|part| part.contains("printf native-approval")),
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

    let second = responses.requests()[1].body_json();
    let tool_result = tool_result_block(&second, "toolu_native_bash_approval");
    assert_eq!(tool_result["is_error"].as_bool(), Some(true));

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
            environments: Some(vec![local(test.config.cwd.clone())]),
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
            items: vec![UserInput::Text {
                text: "trigger repeated pause_turn".into(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
            additional_context: Default::default(),
            thread_settings: Default::default(),
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn claude_wire_context_window_overflow_auto_compacts_and_retries() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let prompt = "recover a full Claude context";
    let follow_up_prompt = "short follow-up after compact";
    let responses = mount_claude_sse_sequence(
        &server,
        vec![
            claude_context_window_exceeded_sse(),
            claude_text_sse("msg_compact", "summary after compact"),
            claude_text_sse("msg_2", "done after compact"),
            claude_text_sse("msg_3", "done after follow-up"),
        ],
    )
    .await;
    let count_tokens = mount_claude_count_tokens_response(
        &server,
        ResponseTemplate::new(200).set_body_json(json!({ "input_tokens": 123 })),
        2,
    )
    .await;

    let test = test_codex()
        .with_config(|config| {
            configure_claude_provider_without_stream_retries(config);
            config.model_auto_compact_token_limit = Some(10_000);
        })
        .build(&server)
        .await?;

    submit_user_input_without_waiting(&test, prompt).await?;

    let first_turn_events = collect_events_until_turn_complete(&test).await;
    let context_window_errors = count_context_window_errors(&first_turn_events);
    assert_eq!(
        context_window_errors, 0,
        "successful recovery should not report a terminal context-window error"
    );
    let first_turn_usage = token_usage_with_source(
        &first_turn_events,
        ContextTokenUsageSource::ClaudeCountTokens,
    );
    assert_eq!(first_turn_usage.context_tokens, Some(123));
    assert_eq!(
        first_turn_usage.context_source,
        Some(ContextTokenUsageSource::ClaudeCountTokens)
    );

    submit_user_input_without_waiting(&test, follow_up_prompt).await?;
    let second_turn_events = collect_events_until_turn_complete(&test).await;
    assert_eq!(
        count_context_window_errors(&second_turn_events),
        0,
        "post-compact follow-up should not reuse stale pre-compaction occupancy"
    );
    let second_turn_usage = token_usage_with_source(
        &second_turn_events,
        ContextTokenUsageSource::ClaudeCountTokens,
    );
    assert_eq!(second_turn_usage.context_tokens, Some(123));

    let requests = responses.requests();
    assert_eq!(
        requests.len(),
        4,
        "expected first request, compact request, retried request, and follow-up request"
    );

    let initial_request = requests[0].body_json();
    let compact_request = requests[1].body_json();
    let retry_request = requests[2].body_json();
    let follow_up_request = requests[3].body_json();
    assert_claude_request_contains(&initial_request, prompt, "initial Claude request");
    assert_claude_request_excludes(
        &initial_request,
        compact_prompt_head(),
        "initial Claude request",
    );
    assert_claude_request_excludes(
        &initial_request,
        "summary after compact",
        "initial Claude request",
    );
    assert_claude_request_contains(&compact_request, prompt, "recovery compact request");
    assert_claude_request_contains(
        &compact_request,
        compact_prompt_head(),
        "recovery compact request",
    );
    assert_claude_request_excludes(
        &compact_request,
        "summary after compact",
        "recovery compact request",
    );
    assert_claude_request_contains(&retry_request, prompt, "retried Claude request");
    assert_claude_request_contains(
        &retry_request,
        SUMMARY_PREFIX.trim(),
        "retried Claude request",
    );
    assert_claude_request_contains(
        &retry_request,
        "summary after compact",
        "retried Claude request",
    );
    assert_claude_request_excludes(
        &retry_request,
        compact_prompt_head(),
        "retried Claude request",
    );
    assert_claude_request_contains(
        &follow_up_request,
        follow_up_prompt,
        "post-compact follow-up request",
    );
    assert_claude_request_contains(
        &follow_up_request,
        "summary after compact",
        "post-compact follow-up request",
    );
    assert_claude_request_excludes(
        &follow_up_request,
        compact_prompt_head(),
        "post-compact follow-up request",
    );

    let count_requests = count_tokens.requests();
    assert_eq!(count_requests.len(), 2);
    let first_count_request = count_requests[0].body_json();
    assert_claude_request_contains(
        &first_count_request,
        "summary after compact",
        "first count_tokens request",
    );
    assert_claude_request_contains(
        &first_count_request,
        "done after compact",
        "first count_tokens request",
    );
    assert_claude_request_excludes(
        &first_count_request,
        compact_prompt_head(),
        "first count_tokens request",
    );
    let second_count_request = count_requests[1].body_json();
    assert_claude_request_contains(
        &second_count_request,
        follow_up_prompt,
        "second count_tokens request",
    );
    assert_claude_request_contains(
        &second_count_request,
        "summary after compact",
        "second count_tokens request",
    );
    assert_claude_request_excludes(
        &second_count_request,
        compact_prompt_head(),
        "second count_tokens request",
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn claude_wire_context_window_retry_overflow_reports_terminal_error() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let prompt = "fail recovery after a second overflow";
    let responses = mount_claude_sse_sequence(
        &server,
        vec![
            claude_context_window_exceeded_sse(),
            claude_text_sse("msg_compact", "summary after compact"),
            claude_context_window_exceeded_sse(),
        ],
    )
    .await;
    mount_claude_count_tokens_never(&server).await;

    let test = test_codex()
        .with_config(configure_claude_provider_without_stream_retries)
        .build(&server)
        .await?;

    submit_user_input_without_waiting(&test, prompt).await?;

    let errors = collect_error_infos_until_turn_complete(&test).await;
    let context_window_errors = errors
        .iter()
        .filter(|info| info.as_ref() == Some(&CodexErrorInfo::ContextWindowExceeded))
        .count();
    assert_eq!(
        context_window_errors, 1,
        "repeated overflow should report exactly one terminal context-window error"
    );
    let requests = responses.requests();
    assert_eq!(
        requests.len(),
        3,
        "expected one compact attempt and one retried sampling request"
    );
    let compact_request = requests[1].body_json();
    let retry_request = requests[2].body_json();
    assert_claude_request_contains(&compact_request, prompt, "recovery compact request");
    assert_claude_request_contains(
        &compact_request,
        compact_prompt_head(),
        "recovery compact request",
    );
    assert_claude_request_contains(&retry_request, prompt, "retried Claude request");
    assert_claude_request_contains(
        &retry_request,
        "summary after compact",
        "retried Claude request",
    );
    assert_claude_request_excludes(
        &retry_request,
        compact_prompt_head(),
        "retried Claude request",
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn claude_wire_context_window_compact_failure_reports_terminal_error() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let prompt = "fail compaction during recovery";
    let responses = mount_claude_sse_sequence(
        &server,
        vec![
            claude_context_window_exceeded_sse(),
            claude_provider_error_sse("compact failed"),
        ],
    )
    .await;
    mount_claude_count_tokens_never(&server).await;

    let test = test_codex()
        .with_config(configure_claude_provider_without_stream_retries)
        .build(&server)
        .await?;

    submit_user_input_without_waiting(&test, prompt).await?;

    let errors = collect_error_infos_until_turn_complete(&test).await;
    let context_window_errors = errors
        .iter()
        .filter(|info| info.as_ref() == Some(&CodexErrorInfo::ContextWindowExceeded))
        .count();
    assert_eq!(
        context_window_errors, 1,
        "compact failure should report a terminal context-window error"
    );
    let requests = responses.requests();
    assert_eq!(
        requests.len(),
        2,
        "compact failure should not retry the original turn"
    );
    let compact_request = requests[1].body_json();
    assert_claude_request_contains(&compact_request, prompt, "recovery compact request");
    assert_claude_request_contains(
        &compact_request,
        compact_prompt_head(),
        "recovery compact request",
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn claude_wire_context_window_admission_compacts_projected_input_before_sampling()
-> anyhow::Result<()> {
    let server = start_mock_server().await;
    let prompt = "large projected admission input";
    let responses = mount_claude_sse_sequence(
        &server,
        vec![
            claude_text_sse("msg_admission_compact", "admission summary"),
            claude_text_sse("msg_2", "done after admission compact"),
        ],
    )
    .await;
    let count_tokens = mount_claude_count_tokens_response(
        &server,
        ResponseTemplate::new(200).set_body_json(json!({ "input_tokens": 123 })),
        1,
    )
    .await;

    let test = test_codex()
        .with_config(|config| {
            configure_claude_provider_without_stream_retries(config);
            config.model_auto_compact_token_limit_scope =
                AutoCompactTokenLimitScope::BodyAfterPrefix;
            config.model_auto_compact_token_limit = Some(1);
        })
        .build(&server)
        .await?;

    submit_user_input_without_waiting(&test, prompt).await?;

    let events = collect_events_until_turn_complete(&test).await;
    let context_window_errors = count_context_window_errors(&events);
    assert_eq!(
        context_window_errors, 0,
        "admission compaction should recover before a provider overflow"
    );
    let usage = token_usage_with_source(&events, ContextTokenUsageSource::ClaudeCountTokens);
    assert_eq!(usage.context_tokens, Some(123));

    let requests = responses.requests();
    assert_eq!(
        requests.len(),
        2,
        "projected input should compact before the first sampling request"
    );
    let compact_request = requests[0].body_json();
    let sampling_request = requests[1].body_json();
    assert_claude_request_contains(&compact_request, prompt, "admission compact request");
    assert_claude_request_contains(
        &compact_request,
        compact_prompt_head(),
        "admission compact request",
    );
    assert_claude_request_contains(&sampling_request, prompt, "post-admission sampling request");
    assert_claude_request_contains(
        &sampling_request,
        SUMMARY_PREFIX.trim(),
        "post-admission sampling request",
    );
    assert_claude_request_contains(
        &sampling_request,
        "admission summary",
        "post-admission sampling request",
    );
    assert_claude_request_excludes(
        &sampling_request,
        compact_prompt_head(),
        "post-admission sampling request",
    );

    let count_request = count_tokens.single_request().body_json();
    assert_claude_request_contains(
        &count_request,
        "admission summary",
        "admission count_tokens request",
    );
    assert_claude_request_contains(
        &count_request,
        "done after admission compact",
        "admission count_tokens request",
    );
    assert_claude_request_excludes(
        &count_request,
        compact_prompt_head(),
        "admission count_tokens request",
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn claude_wire_context_window_recovery_retries_before_pending_steer() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let original_prompt = "recover before queued steer";
    let pending_steer = "queued steer should wait";
    let responses = mount_claude_sse_sequence_with_delays(
        &server,
        vec![
            (claude_context_window_exceeded_sse(), None),
            (
                claude_text_sse("msg_compact", "summary before queued steer"),
                Some(Duration::from_millis(200)),
            ),
            (claude_text_sse("msg_2", "done after recovery retry"), None),
            (claude_text_sse("msg_3", "done after queued steer"), None),
        ],
    )
    .await;
    mount_claude_count_tokens_response(
        &server,
        ResponseTemplate::new(200).set_body_json(json!({ "input_tokens": 123 })),
        1,
    )
    .await;

    let test = test_codex()
        .with_config(configure_claude_provider_without_stream_retries)
        .build(&server)
        .await?;

    submit_user_input_without_waiting(&test, original_prompt).await?;
    wait_for_event(&test.codex, |event| {
        matches!(
            event,
            EventMsg::ItemStarted(item) if matches!(item.item, TurnItem::ContextCompaction(_))
        )
    })
    .await;
    submit_user_input_without_waiting(&test, pending_steer).await?;

    let context_window_errors = count_context_window_errors_until_turn_complete(&test).await;
    assert_eq!(
        context_window_errors, 0,
        "queued steer should not turn successful recovery into a terminal context-window error"
    );

    let requests = responses.requests();
    assert_eq!(
        requests.len(),
        4,
        "expected initial overflow, compact, retry of original turn, and then queued steer"
    );
    let retry_request = requests[2].body_json();
    let queued_steer_request = requests[3].body_json();
    assert_claude_request_contains(&retry_request, original_prompt, "recovery retry request");
    assert_claude_request_contains(
        &retry_request,
        "summary before queued steer",
        "recovery retry request",
    );
    assert_claude_request_excludes(&retry_request, pending_steer, "recovery retry request");
    assert_claude_request_contains(
        &queued_steer_request,
        original_prompt,
        "queued steer follow-up request",
    );
    assert_claude_request_contains(
        &queued_steer_request,
        pending_steer,
        "queued steer follow-up request",
    );

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

fn configure_claude_provider_without_stream_retries(config: &mut Config) {
    configure_claude_provider(config);
    config.model_provider.stream_max_retries = Some(0);
}

fn configure_claude_provider_with_web_search_context_size(config: &mut Config) {
    configure_claude_provider(config);
    config
        .web_search_mode
        .set(WebSearchMode::Live)
        .unwrap_or_else(|err| panic!("test web_search_mode should satisfy constraints: {err}"));
    config.web_search_config = Some(WebSearchConfig {
        search_context_size: Some(WebSearchContextSize::High),
        ..WebSearchConfig::default()
    });
}

fn configure_compatible_claude_provider(config: &mut Config) {
    configure_claude_provider(config);
    config.model_provider.name = "aicodex_gateway_claude".to_string();
}

fn configure_compatible_claude_provider_with_fallback_model(config: &mut Config) {
    configure_compatible_claude_provider(config);
    config.model = Some("deepseek-v4-pro".to_string());
}

fn configure_claude_provider_with_apply_patch(config: &mut Config) {
    configure_claude_provider(config);
    config.model = Some("gpt-5.4".to_string());
}

fn configure_claude_provider_with_apply_patch_streaming(config: &mut Config) {
    configure_claude_provider_with_apply_patch(config);
    if let Err(error) = config.features.enable(Feature::ApplyPatchStreamingEvents) {
        panic!("ApplyPatchStreamingEvents should be enableable in tests: {error:?}");
    }
}

fn configure_claude_provider_with_code_mode(config: &mut Config) {
    configure_claude_provider(config);
    if let Err(error) = config.features.enable(Feature::CodeMode) {
        panic!("Code Mode should be enableable in tests: {error:?}");
    }
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

fn claude_custom_tool_use_sse(tool_id: &str, name: &str, input: Value) -> String {
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
                "id": tool_id,
                "name": name,
                "input": input
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

fn claude_custom_tool_use_delta_sse(tool_id: &str, name: &str, partial_json: &str) -> String {
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
                "id": tool_id,
                "name": name,
                "input": {}
            }
        }),
        json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": {
                "type": "input_json_delta",
                "partial_json": partial_json
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

fn claude_context_window_exceeded_sse() -> String {
    sse(vec![
        json!({
            "type": "message_start",
            "message": {
                "id": "msg_context_full",
                "type": "message",
                "role": "assistant",
                "content": [],
                "usage": {"input_tokens": 1, "output_tokens": 0}
            }
        }),
        json!({
            "type": "message_delta",
            "delta": {"stop_reason": "model_context_window_exceeded"},
            "usage": {"output_tokens": 0}
        }),
        claude_message_stop(),
    ])
}

fn claude_provider_error_sse(message: &str) -> String {
    sse(vec![json!({
        "type": "error",
        "error": {
            "type": "api_error",
            "message": message
        }
    })])
}

fn claude_text_sse(message_id: &str, text: &str) -> String {
    claude_text_sse_with_usage(message_id, text, 1, 1)
}

fn claude_text_sse_with_usage(
    message_id: &str,
    text: &str,
    input_tokens: i64,
    output_tokens: i64,
) -> String {
    sse(vec![
        json!({
            "type": "message_start",
            "message": {
                "id": message_id,
                "type": "message",
                "role": "assistant",
                "content": [],
                "usage": {"input_tokens": input_tokens, "output_tokens": output_tokens}
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

fn tool_result_block(body: &Value, tool_use_id: &str) -> Value {
    message_content_blocks(body)
        .into_iter()
        .find(|block| {
            block.get("type").and_then(Value::as_str) == Some("tool_result")
                && block.get("tool_use_id").and_then(Value::as_str) == Some(tool_use_id)
        })
        .unwrap_or_else(|| {
            panic!("Claude request should include tool_result {tool_use_id}: {body}")
        })
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
