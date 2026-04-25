use codex_model_provider_info::ModelProviderInfo;
use codex_model_provider_info::WireApi;
use codex_protocol::config_types::ServiceTier;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::Op;
use codex_protocol::user_input::UserInput;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::test_codex;
use core_test_support::wait_for_event;
use core_test_support::wait_for_event_match;
use core_test_support::wait_for_event_with_timeout;
use pretty_assertions::assert_eq;
use serde_json::json;
use tokio::time::Duration;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::Request;
use wiremock::ResponseTemplate;
use wiremock::matchers::body_string_contains;
use wiremock::matchers::method;
use wiremock::matchers::path;

fn anthropic_provider(base_url: String) -> ModelProviderInfo {
    ModelProviderInfo {
        name: "Anthropic".to_string(),
        base_url: Some(base_url),
        env_key: None,
        env_key_instructions: None,
        experimental_bearer_token: Some("test-token".to_string()),
        auth: None,
        wire_api: WireApi::Anthropic,
        query_params: None,
        http_headers: None,
        env_http_headers: None,
        request_max_retries: Some(0),
        stream_max_retries: Some(0),
        stream_idle_timeout_ms: Some(5_000),
        websocket_connect_timeout_ms: None,
        requires_openai_auth: false,
        supports_websockets: false,
    }
}

fn anthropic_sse(events: Vec<serde_json::Value>) -> String {
    events
        .into_iter()
        .map(|event| {
            let event_name = event
                .get("type")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("message_start");
            format!("event: {event_name}\ndata: {event}\n\n")
        })
        .collect::<String>()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anthropic_text_stream_round_trip() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let server = MockServer::start().await;
    let sse = anthropic_sse(vec![
        json!({
            "type":"message_start",
            "message":{
                "id":"resp-1",
                "type":"message",
                "role":"assistant",
                "content":[]
            }
        }),
        json!({
            "type":"content_block_delta",
            "index":0,
            "delta":{"type":"text_delta","text":"hello from anthropic"}
        }),
        json!({
            "type":"message_delta",
            "delta":{"stop_reason":"end_turn","stop_sequence":null},
            "usage":{"input_tokens":4,"output_tokens":3}
        }),
        json!({"type":"message_stop"}),
    ]);

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(sse, "text/event-stream"),
        )
        .mount(&server)
        .await;

    let test = test_codex()
        .with_config({
            let provider = anthropic_provider(server.uri());
            move |config| {
                config.model_provider = provider;
                config.model_provider_id = "anthropic".to_string();
                config.model = Some("claude-test".to_string());
            }
        })
        .build(&server)
        .await?;

    test.codex
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: "hello".to_string(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
        })
        .await?;

    let terminal = wait_for_event_with_timeout(
        &test.codex,
        |event| {
            matches!(
                event,
                EventMsg::AgentMessage(_) | EventMsg::Error(_) | EventMsg::TurnComplete(_)
            )
        },
        Duration::from_secs(60),
    )
    .await;
    match terminal {
        EventMsg::AgentMessage(message) => {
            assert_eq!(message.message, "hello from anthropic");
        }
        EventMsg::Error(error) => {
            panic!("anthropic text stream failed: {}", error.message);
        }
        EventMsg::TurnComplete(_) => {
            panic!("turn completed before assistant produced text");
        }
        _ => unreachable!(),
    }

    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anthropic_reasoning_stream_round_trip() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let server = MockServer::start().await;
    let sse = anthropic_sse(vec![
        json!({
            "type":"message_start",
            "message":{
                "id":"resp-reasoning-1",
                "type":"message",
                "role":"assistant",
                "content":[]
            }
        }),
        json!({
            "type":"content_block_start",
            "index":0,
            "content_block":{"type":"thinking","thinking":"","signature":""}
        }),
        json!({
            "type":"content_block_delta",
            "index":0,
            "delta":{"type":"thinking_delta","thinking":"step one"}
        }),
        json!({
            "type":"content_block_delta",
            "index":0,
            "delta":{"type":"signature_delta","signature":"sig-1"}
        }),
        json!({
            "type":"content_block_stop",
            "index":0
        }),
        json!({
            "type":"content_block_delta",
            "index":1,
            "delta":{"type":"text_delta","text":"done"}
        }),
        json!({
            "type":"message_delta",
            "delta":{"stop_reason":"end_turn","stop_sequence":null},
            "usage":{"input_tokens":4,"output_tokens":3}
        }),
        json!({"type":"message_stop"}),
    ]);

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(sse, "text/event-stream"),
        )
        .mount(&server)
        .await;

    let test = test_codex()
        .with_config({
            let provider = anthropic_provider(server.uri());
            move |config| {
                config.model_provider = provider;
                config.model_provider_id = "anthropic".to_string();
                config.model = Some("claude-test".to_string());
            }
        })
        .build(&server)
        .await?;

    test.codex
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: "reason".to_string(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
        })
        .await?;

    let reasoning = wait_for_event_match(&test.codex, |event| match event {
        EventMsg::ReasoningContentDelta(delta) => Some(delta.clone()),
        _ => None,
    })
    .await;
    assert_eq!(reasoning.delta, "step one");

    let terminal = {
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            let event = match tokio::time::timeout(
                remaining.max(Duration::from_secs(1)),
                test.codex.next_event(),
            )
            .await
            {
                Ok(Ok(event)) => event.msg,
                Ok(Err(err)) => panic!("event stream ended unexpectedly: {err}"),
                Err(_) => {
                    let requests = server
                        .received_requests()
                        .await
                        .unwrap_or_default()
                        .into_iter()
                        .map(|req| String::from_utf8_lossy(&req.body).to_string())
                        .collect::<Vec<_>>();
                    panic!(
                        "timeout waiting for apply_patch terminal event; requests={requests:#?}"
                    );
                }
            };
            if matches!(
                event,
                EventMsg::AgentMessage(_) | EventMsg::Error(_) | EventMsg::TurnComplete(_)
            ) {
                break event;
            }
        }
    };
    match terminal {
        EventMsg::AgentMessage(message) => assert_eq!(message.message, "done"),
        EventMsg::Error(error) => panic!("anthropic reasoning stream failed: {}", error.message),
        EventMsg::TurnComplete(_) => panic!("turn completed before assistant produced text"),
        _ => unreachable!(),
    }

    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anthropic_tool_use_round_trip() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let server = MockServer::start().await;

    let first = anthropic_sse(vec![
        json!({
            "type":"message_start",
            "message":{
                "id":"resp-tool-1",
                "type":"message",
                "role":"assistant",
                "content":[]
            }
        }),
        json!({
            "type":"content_block_start",
            "index":0,
            "content_block":{
                "type":"tool_use",
                "id":"call_time_1",
                "name":"time",
                "input":{"utc_offset":"+00:00"}
            }
        }),
        json!({
            "type":"content_block_stop",
            "index":0
        }),
        json!({
            "type":"message_delta",
            "delta":{"stop_reason":"tool_use","stop_sequence":null},
            "usage":{"input_tokens":7,"output_tokens":4}
        }),
        json!({"type":"message_stop"}),
    ]);
    let first_request_matcher = |req: &Request| {
        let body = String::from_utf8_lossy(&req.body);
        body.contains("tell utc time") && !body.contains("\"type\":\"tool_result\"")
    };
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(first_request_matcher)
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(first, "text/event-stream"),
        )
        .mount(&server)
        .await;

    let second = anthropic_sse(vec![
        json!({
            "type":"message_start",
            "message":{
                "id":"resp-tool-2",
                "type":"message",
                "role":"assistant",
                "content":[]
            }
        }),
        json!({
            "type":"content_block_delta",
            "index":0,
            "delta":{"type":"text_delta","text":"UTC_OK"}
        }),
        json!({
            "type":"message_delta",
            "delta":{"stop_reason":"end_turn","stop_sequence":null},
            "usage":{"input_tokens":14,"output_tokens":3}
        }),
        json!({"type":"message_stop"}),
    ]);
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(body_string_contains("\"type\":\"tool_result\""))
        .and(body_string_contains("\"tool_use_id\":\"call_time_1\""))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(second, "text/event-stream"),
        )
        .mount(&server)
        .await;

    let test = test_codex()
        .with_config({
            let provider = anthropic_provider(server.uri());
            move |config| {
                config.model_provider = provider;
                config.model_provider_id = "anthropic".to_string();
                config.model = Some("claude-test".to_string());
            }
        })
        .build(&server)
        .await?;

    test.codex
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: "tell utc time".to_string(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
        })
        .await?;

    let terminal = wait_for_event_with_timeout(
        &test.codex,
        |event| {
            matches!(
                event,
                EventMsg::AgentMessage(_) | EventMsg::Error(_) | EventMsg::TurnComplete(_)
            )
        },
        Duration::from_secs(20),
    )
    .await;
    match terminal {
        EventMsg::AgentMessage(message) => assert_eq!(message.message, "UTC_OK"),
        EventMsg::Error(error) => panic!("anthropic tool round trip failed: {}", error.message),
        EventMsg::TurnComplete(_) => panic!("turn completed before assistant produced UTC_OK"),
        _ => unreachable!(),
    }

    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anthropic_local_shell_round_trip() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let server = MockServer::start().await;

    let first = anthropic_sse(vec![
        json!({
            "type":"message_start",
            "message":{
                "id":"resp-shell-1",
                "type":"message",
                "role":"assistant",
                "content":[]
            }
        }),
        json!({
            "type":"content_block_start",
            "index":0,
            "content_block":{
                "type":"tool_use",
                "id":"call_shell_1",
                "name":"local_shell",
                "input":{"command":["pwd"],"workdir":".","timeout_ms":1000}
            }
        }),
        json!({
            "type":"content_block_stop",
            "index":0
        }),
        json!({
            "type":"message_delta",
            "delta":{"stop_reason":"tool_use","stop_sequence":null},
            "usage":{"input_tokens":7,"output_tokens":4}
        }),
        json!({"type":"message_stop"}),
    ]);
    let first_request_matcher = |req: &Request| {
        let body = String::from_utf8_lossy(&req.body);
        body.contains("run local shell") && !body.contains("\"tool_use_id\":\"call_shell_1\"")
    };
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(first_request_matcher)
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(first, "text/event-stream"),
        )
        .mount(&server)
        .await;

    let second = anthropic_sse(vec![
        json!({
            "type":"message_start",
            "message":{
                "id":"resp-shell-2",
                "type":"message",
                "role":"assistant",
                "content":[]
            }
        }),
        json!({
            "type":"content_block_delta",
            "index":0,
            "delta":{"type":"text_delta","text":"SHELL_OK"}
        }),
        json!({
            "type":"message_delta",
            "delta":{"stop_reason":"end_turn","stop_sequence":null},
            "usage":{"input_tokens":14,"output_tokens":3}
        }),
        json!({"type":"message_stop"}),
    ]);
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(body_string_contains("\"tool_use_id\":\"call_shell_1\""))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(second, "text/event-stream"),
        )
        .mount(&server)
        .await;

    let test = test_codex()
        .with_config({
            let provider = anthropic_provider(server.uri());
            move |config| {
                config.model_provider = provider;
                config.model_provider_id = "anthropic".to_string();
                config.model = Some("claude-test".to_string());
            }
        })
        .build(&server)
        .await?;

    test.codex
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: "run local shell".to_string(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
        })
        .await?;

    let terminal = wait_for_event_with_timeout(
        &test.codex,
        |event| {
            matches!(
                event,
                EventMsg::AgentMessage(_) | EventMsg::Error(_) | EventMsg::TurnComplete(_)
            )
        },
        Duration::from_secs(20),
    )
    .await;
    match terminal {
        EventMsg::AgentMessage(message) => assert_eq!(message.message, "SHELL_OK"),
        EventMsg::Error(error) => panic!("anthropic local shell failed: {}", error.message),
        EventMsg::TurnComplete(_) => panic!("turn completed before assistant produced SHELL_OK"),
        _ => unreachable!(),
    }

    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anthropic_apply_patch_round_trip() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let server = MockServer::start().await;
    let file_name = "anthropic_apply_patch.txt";
    let patch = format!(
        "*** Begin Patch\n*** Add File: {file_name}\n+hello from anthropic apply_patch\n*** End Patch\n"
    );

    let first = anthropic_sse(vec![
        json!({
            "type":"message_start",
            "message":{
                "id":"resp-patch-1",
                "type":"message",
                "role":"assistant",
                "content":[]
            }
        }),
        json!({
            "type":"content_block_start",
            "index":0,
            "content_block":{
                "type":"tool_use",
                "id":"call_patch_1",
                "name":"apply_patch",
                "input":{"input":patch}
            }
        }),
        json!({
            "type":"content_block_stop",
            "index":0
        }),
        json!({
            "type":"message_delta",
            "delta":{"stop_reason":"tool_use","stop_sequence":null},
            "usage":{"input_tokens":7,"output_tokens":4}
        }),
        json!({"type":"message_stop"}),
    ]);
    let first_request_matcher = |req: &Request| {
        let body = String::from_utf8_lossy(&req.body);
        body.contains("apply patch") && !body.contains("\"tool_use_id\":\"call_patch_1\"")
    };
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(first_request_matcher)
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(first, "text/event-stream"),
        )
        .mount(&server)
        .await;

    let second = anthropic_sse(vec![
        json!({
            "type":"message_start",
            "message":{
                "id":"resp-patch-2",
                "type":"message",
                "role":"assistant",
                "content":[]
            }
        }),
        json!({
            "type":"content_block_delta",
            "index":0,
            "delta":{"type":"text_delta","text":"PATCH_OK"}
        }),
        json!({
            "type":"message_delta",
            "delta":{"stop_reason":"end_turn","stop_sequence":null},
            "usage":{"input_tokens":14,"output_tokens":3}
        }),
        json!({"type":"message_stop"}),
    ]);
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(body_string_contains("\"tool_use_id\":\"call_patch_1\""))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(second, "text/event-stream"),
        )
        .mount(&server)
        .await;

    let test = test_codex()
        .with_config({
            let provider = anthropic_provider(server.uri());
            move |config| {
                config.model_provider = provider;
                config.model_provider_id = "anthropic".to_string();
                config.model = Some("claude-test".to_string());
                config.include_apply_patch_tool = true;
            }
        })
        .build(&server)
        .await?;

    test.codex
        .submit(Op::UserTurn {
            items: vec![UserInput::Text {
                text: "apply patch".to_string(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            cwd: test.config.cwd.to_path_buf(),
            approval_policy: codex_protocol::protocol::AskForApproval::Never,
            approvals_reviewer: None,
            sandbox_policy: codex_protocol::protocol::SandboxPolicy::DangerFullAccess,
            model: test.session_configured.model.clone(),
            effort: None,
            summary: None,
            service_tier: None,
            collaboration_mode: None,
            personality: None,
        })
        .await?;

    let terminal = wait_for_event_with_timeout(
        &test.codex,
        |event| {
            matches!(
                event,
                EventMsg::AgentMessage(_) | EventMsg::Error(_) | EventMsg::TurnComplete(_)
            )
        },
        Duration::from_secs(20),
    )
    .await;
    match terminal {
        EventMsg::AgentMessage(message) => assert_eq!(message.message, "PATCH_OK"),
        EventMsg::Error(error) => panic!("anthropic apply_patch failed: {}", error.message),
        EventMsg::TurnComplete(_) => panic!("turn completed before assistant produced PATCH_OK"),
        _ => unreachable!(),
    }

    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    let content = std::fs::read_to_string(test.workspace_path(file_name))?;
    assert_eq!(content, "hello from anthropic apply_patch\n");

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anthropic_401_maps_to_unexpected_status_error() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(401)
                .insert_header("content-type", "application/json")
                .set_body_json(json!({
                    "type": "error",
                    "error": {
                        "type": "authentication_error",
                        "message": "bad auth"
                    }
                })),
        )
        .mount(&server)
        .await;

    let test = test_codex()
        .with_config({
            let provider = anthropic_provider(server.uri());
            move |config| {
                config.model_provider = provider;
                config.model_provider_id = "anthropic".to_string();
                config.model = Some("claude-test".to_string());
            }
        })
        .build(&server)
        .await?;

    test.codex
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: "hello".to_string(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
        })
        .await?;

    let error = wait_for_event_match(&test.codex, |event| match event {
        EventMsg::Error(error) => Some(error.clone()),
        _ => None,
    })
    .await;
    assert!(error.message.contains("unexpected status 401"));
    assert!(error.message.contains("bad auth"));

    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anthropic_429_maps_to_retry_limit_error() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("content-type", "application/json")
                .set_body_json(json!({
                    "type": "error",
                    "error": {
                        "type": "rate_limit_error",
                        "message": "slow down"
                    }
                })),
        )
        .mount(&server)
        .await;

    let test = test_codex()
        .with_config({
            let provider = anthropic_provider(server.uri());
            move |config| {
                config.model_provider = provider;
                config.model_provider_id = "anthropic".to_string();
                config.model = Some("claude-test".to_string());
            }
        })
        .build(&server)
        .await?;

    test.codex
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: "hello".to_string(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
        })
        .await?;

    let error = wait_for_event_match(&test.codex, |event| match event {
        EventMsg::Error(error) => Some(error.clone()),
        _ => None,
    })
    .await;
    assert!(error.message.contains("exceeded retry limit"));
    assert!(error.message.contains("429"));

    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anthropic_500_maps_to_internal_server_error() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(500)
                .insert_header("content-type", "application/json")
                .set_body_json(json!({
                    "type": "error",
                    "error": {
                        "type": "api_error",
                        "message": "backend exploded"
                    }
                })),
        )
        .mount(&server)
        .await;

    let test = test_codex()
        .with_config({
            let provider = anthropic_provider(server.uri());
            move |config| {
                config.model_provider = provider;
                config.model_provider_id = "anthropic".to_string();
                config.model = Some("claude-test".to_string());
            }
        })
        .build(&server)
        .await?;

    test.codex
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: "hello".to_string(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
        })
        .await?;

    let error = wait_for_event_match(&test.codex, |event| match event {
        EventMsg::Error(error) => Some(error.clone()),
        _ => None,
    })
    .await;
    assert!(
        error
            .message
            .contains("We're currently experiencing high demand")
    );

    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anthropic_request_exposes_web_search() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let server = MockServer::start().await;
    let sse = anthropic_sse(vec![
        json!({
            "type":"message_start",
            "message":{
                "id":"resp-filter-1",
                "type":"message",
                "role":"assistant",
                "content":[]
            }
        }),
        json!({
            "type":"content_block_delta",
            "index":0,
            "delta":{"type":"text_delta","text":"ok"}
        }),
        json!({
            "type":"message_delta",
            "delta":{"stop_reason":"end_turn","stop_sequence":null},
            "usage":{"input_tokens":4,"output_tokens":1}
        }),
        json!({"type":"message_stop"}),
    ]);

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(sse, "text/event-stream"),
        )
        .mount(&server)
        .await;

    let test = test_codex()
        .with_config({
            let provider = anthropic_provider(server.uri());
            move |config| {
                config.model_provider = provider;
                config.model_provider_id = "anthropic".to_string();
                config.model = Some("claude-test".to_string());
            }
        })
        .build(&server)
        .await?;

    test.codex
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: "hello".to_string(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
        })
        .await?;

    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    let requests = server.received_requests().await.unwrap_or_default();
    let body = serde_json::from_slice::<serde_json::Value>(&requests[0].body)?;
    let tools = body["tools"].as_array().cloned().unwrap_or_default();
    assert!(
        tools.iter().any(|tool| {
            tool["name"] == json!("web_search") || tool["type"] == json!("web_search_20250305")
        }),
        "Anthropic prompt tools should expose web_search server tool"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anthropic_request_replays_image_generation_history() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let server = MockServer::start().await;
    let first = anthropic_sse(vec![
        json!({
            "type":"message_start",
            "message":{
                "id":"resp-image-1",
                "type":"message",
                "role":"assistant",
                "content":[]
            }
        }),
        json!({
            "type":"content_block_start",
            "index":0,
            "content_block":{
                "type":"server_tool_use",
                "id":"srvtoolu_img_1",
                "name":"image_generation",
                "input":{"prompt":"a red kite"}
            }
        }),
        json!({"type":"content_block_stop","index":0}),
        json!({
            "type":"content_block_start",
            "index":1,
            "content_block":{
                "type":"image_generation_tool_result",
                "tool_use_id":"srvtoolu_img_1",
                "revised_prompt":"a bright red kite over water",
                "result":"Zm9v",
                "content":[{
                    "type":"image",
                    "source":{"type":"base64","media_type":"image/png","data":"Zm9v"}
                }]
            }
        }),
        json!({"type":"content_block_stop","index":1}),
        json!({
            "type":"message_delta",
            "delta":{"stop_reason":"end_turn","stop_sequence":null},
            "usage":{"input_tokens":4,"output_tokens":1}
        }),
        json!({"type":"message_stop"}),
    ]);

    let second = anthropic_sse(vec![
        json!({
            "type":"message_start",
            "message":{
                "id":"resp-image-2",
                "type":"message",
                "role":"assistant",
                "content":[]
            }
        }),
        json!({
            "type":"content_block_delta",
            "index":0,
            "delta":{"type":"text_delta","text":"done"}
        }),
        json!({
            "type":"message_delta",
            "delta":{"stop_reason":"end_turn","stop_sequence":null},
            "usage":{"input_tokens":4,"output_tokens":1}
        }),
        json!({"type":"message_stop"}),
    ]);

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(first, "text/event-stream"),
        )
        .up_to_n_times(1)
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(second, "text/event-stream"),
        )
        .up_to_n_times(1)
        .mount(&server)
        .await;

    let test = test_codex()
        .with_config({
            let provider = anthropic_provider(server.uri());
            move |config| {
                config.model_provider = provider;
                config.model_provider_id = "anthropic".to_string();
                config.model = Some("claude-test".to_string());
            }
        })
        .build(&server)
        .await?;

    test.codex
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: "generate image".to_string(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
        })
        .await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    test.codex
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: "describe it".to_string(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
        })
        .await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    let requests = server.received_requests().await.unwrap_or_default();
    let second_body = serde_json::from_slice::<serde_json::Value>(&requests[1].body)?;
    let messages = second_body["messages"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let assistant_history = messages
        .iter()
        .find(|message| {
            message["role"] == json!("assistant")
                && message["content"].as_array().is_some_and(|content| {
                    content
                        .iter()
                        .any(|block| block["type"] == json!("image_generation_tool_result"))
                })
        })
        .cloned()
        .expect("second request should replay image generation history");

    assert_eq!(
        assistant_history["content"][0]["type"],
        json!("server_tool_use")
    );
    assert_eq!(
        assistant_history["content"][0]["name"],
        json!("image_generation")
    );
    assert_eq!(
        assistant_history["content"][1]["type"],
        json!("image_generation_tool_result")
    );
    assert_eq!(
        assistant_history["content"][1]["tool_use_id"],
        json!("srvtoolu_img_1")
    );
    assert_eq!(assistant_history["content"][1]["result"], json!("Zm9v"));
    assert_eq!(
        assistant_history["content"][1]["revised_prompt"],
        json!("a bright red kite over water")
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anthropic_service_tier_maps_to_auto() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let server = MockServer::start().await;
    let sse = anthropic_sse(vec![
        json!({
            "type":"message_start",
            "message":{
                "id":"resp-tier-1",
                "type":"message",
                "role":"assistant",
                "content":[]
            }
        }),
        json!({
            "type":"content_block_delta",
            "index":0,
            "delta":{"type":"text_delta","text":"ok"}
        }),
        json!({
            "type":"message_delta",
            "delta":{"stop_reason":"end_turn","stop_sequence":null},
            "usage":{"input_tokens":4,"output_tokens":1}
        }),
        json!({"type":"message_stop"}),
    ]);

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(sse, "text/event-stream"),
        )
        .mount(&server)
        .await;

    let test = test_codex()
        .with_config({
            let provider = anthropic_provider(server.uri());
            move |config| {
                config.model_provider = provider;
                config.model_provider_id = "anthropic".to_string();
                config.model = Some("claude-test".to_string());
            }
        })
        .build(&server)
        .await?;

    test.submit_turn_with_service_tier("hello", Some(ServiceTier::Fast))
        .await?;

    tokio::time::sleep(Duration::from_millis(500)).await;
    let requests = server.received_requests().await.unwrap_or_default();
    assert_eq!(requests.len(), 1, "expected one anthropic request");
    let body = serde_json::from_slice::<serde_json::Value>(&requests[0].body)?;
    assert_eq!(body["service_tier"].as_str(), Some("auto"));

    Ok(())
}
