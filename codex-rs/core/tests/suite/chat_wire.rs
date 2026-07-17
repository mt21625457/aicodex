use codex_core::config::Config;
use codex_model_provider_info::WireApi;
use codex_protocol::dynamic_tools::DynamicToolFunctionSpec;
use codex_protocol::dynamic_tools::DynamicToolSpec;
use codex_protocol::items::TurnItem;
use codex_protocol::protocol::CodexErrorInfo;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::ItemCompletedEvent;
use codex_protocol::protocol::ItemStartedEvent;
use codex_protocol::protocol::Op;
use codex_protocol::protocol::TokenUsage;
use codex_protocol::user_input::UserInput;
use codex_tools::ChatToolCallKind;
use codex_tools::chat_tool_name;
use core_test_support::responses::mount_chat_sse_sequence;
use core_test_support::responses::start_mock_server;
use core_test_support::streaming_sse::StreamingSseChunk;
use core_test_support::streaming_sse::start_streaming_sse_server;
use core_test_support::test_codex::TestCodex;
use core_test_support::test_codex::test_codex;
use core_test_support::wait_for_event;
use core_test_support::wait_for_event_match;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;
use wiremock::Mock;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path;

const EXPECTED_AICODEX_USER_AGENT: &str = concat!("aicodex/", env!("CARGO_PKG_VERSION"));

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_wire_streams_text_on_chat_path_with_uniform_headers() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let responses = mount_chat_sse_sequence(
        &server,
        vec![chat_sse(vec![
            json!({
                "id": "chatcmpl_1",
                "choices": [{"index": 0, "delta": {"role": "assistant"}}]
            }),
            json!({
                "id": "chatcmpl_1",
                "choices": [{
                    "index": 0,
                    "delta": {"content": "hello from chat"},
                    "finish_reason": "stop"
                }]
            }),
            json!({
                "id": "chatcmpl_1",
                "choices": [],
                "usage": {"prompt_tokens": 8, "completion_tokens": 3, "total_tokens": 11}
            }),
        ])],
    )
    .await;
    let test = test_codex()
        .with_model("gpt-5.2")
        .with_config(configure_chat_provider)
        .build_with_auto_env(&server)
        .await?;

    submit_text_turn(&test, "say hello").await?;
    let message = wait_for_event_match(&test.codex, |event| match event {
        EventMsg::AgentMessage(message) => Some(message.message.clone()),
        _ => None,
    })
    .await;
    assert_eq!(message, "hello from chat");
    let usage = wait_for_event_match(&test.codex, |event| match event {
        EventMsg::TokenCount(payload) => payload
            .info
            .as_ref()
            .map(|info| info.last_token_usage.clone()),
        _ => None,
    })
    .await;
    assert_eq!(
        usage,
        TokenUsage {
            input_tokens: 8,
            output_tokens: 3,
            total_tokens: 11,
            ..Default::default()
        }
    );
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    let request = responses.single_request();
    assert_eq!(request.path(), "/v1/chat/completions");
    assert_eq!(
        request.header("accept").as_deref(),
        Some("text/event-stream")
    );
    assert_eq!(
        request.header("authorization").as_deref(),
        Some("Bearer test-token")
    );
    assert_eq!(
        request.header("user-agent").as_deref(),
        Some(EXPECTED_AICODEX_USER_AGENT)
    );
    let body = request.body_json();
    assert_eq!(body["model"], "gpt-5.2");
    assert_eq!(body["stream"], true);
    assert_eq!(body["stream_options"], json!({"include_usage": true}));
    assert!(body["messages"].as_array().is_some_and(|messages| {
        messages
            .iter()
            .any(|message| message["role"] == "user" && message["content"] == "say hello")
    }));
    assert!(body.get("input").is_none());
    assert!(body.get("system").is_none());
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_wire_tool_loop_posts_tool_result_and_completes() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let exec_command_name = chat_tool_name(
        /*namespace*/ None,
        "exec_command",
        ChatToolCallKind::Function,
    );
    let responses = mount_chat_sse_sequence(
        &server,
        vec![
            chat_sse(vec![json!({
                "id": "chatcmpl_tool",
                "choices": [{
                    "index": 0,
                    "delta": {"tool_calls": [{
                        "index": 0,
                        "id": "call_1",
                        "function": {
                            "name": exec_command_name.clone(),
            "arguments": "{\"cmd\":\"echo chat-tool\"}"
                        }
                    }]},
                    "finish_reason": "tool_calls"
                }]
            })]),
            chat_sse(vec![json!({
                "id": "chatcmpl_final",
                "choices": [{
                    "index": 0,
                    "delta": {"content": "done"},
                    "finish_reason": "stop"
                }]
            })]),
        ],
    )
    .await;
    let test = test_codex()
        .with_model("gpt-5.2")
        .with_config(configure_chat_provider)
        .build_with_auto_env(&server)
        .await?;

    submit_text_turn(&test, "run a command").await?;
    let message = wait_for_event_match(&test.codex, |event| match event {
        EventMsg::AgentMessage(message) => Some(message.message.clone()),
        _ => None,
    })
    .await;
    assert_eq!(message, "done");
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    let requests = responses.requests();
    assert_eq!(requests.len(), 2);
    let first = requests[0].body_json();
    assert!(first["tools"].as_array().is_some_and(|tools| {
        tools
            .iter()
            .any(|tool| tool["type"] == "function" && tool["function"]["name"] == exec_command_name)
    }));
    let second = requests[1].body_json();
    let messages = second["messages"].as_array().expect("Chat messages");
    let assistant_call = messages
        .iter()
        .find(|message| message["role"] == "assistant" && message.get("tool_calls").is_some())
        .expect("assistant tool call history");
    assert_eq!(assistant_call["tool_calls"][0]["id"], "call_1");
    let tool_result = messages
        .iter()
        .find(|message| message["role"] == "tool" && message["tool_call_id"] == "call_1")
        .expect("tool result history");
    assert!(
        tool_result["content"]
            .as_str()
            .is_some_and(|content| content.contains("chat-tool"))
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_wire_provider_error_reaches_the_turn() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let responses = mount_chat_sse_sequence(
        &server,
        vec![format!(
            "data: {}\n\n",
            json!({
                "error": {
                    "type": "invalid_request_error",
                    "message": "chat request rejected"
                }
            })
        )],
    )
    .await;
    let test = test_codex()
        .with_model("gpt-5.2")
        .with_config(configure_chat_provider)
        .build_with_auto_env(&server)
        .await?;

    test.codex
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: "fail this turn".to_string(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
            additional_context: Default::default(),
            thread_settings: Default::default(),
        })
        .await?;
    let error = wait_for_event_match(&test.codex, |event| match event {
        EventMsg::Error(error) => Some(error.clone()),
        _ => None,
    })
    .await;
    assert_eq!(error.codex_error_info, Some(CodexErrorInfo::Other));
    assert!(
        error.message.contains("chat request rejected"),
        "unexpected error: {}",
        error.message
    );
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;
    assert_eq!(responses.requests().len(), 1);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_wire_empty_stream_reaches_the_turn() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let responses = mount_chat_sse_sequence(&server, vec![String::new()]).await;
    let test = test_codex()
        .with_model("gpt-5.2")
        .with_config(configure_chat_provider)
        .build_with_auto_env(&server)
        .await?;

    test.codex
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: "return an empty stream".to_string(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
            additional_context: Default::default(),
            thread_settings: Default::default(),
        })
        .await?;
    let error = wait_for_event_match(&test.codex, |event| match event {
        EventMsg::Error(error) => Some(error.clone()),
        _ => None,
    })
    .await;
    assert_eq!(error.codex_error_info, Some(CodexErrorInfo::Other));
    assert!(
        error.message.contains("before a terminal finish reason"),
        "unexpected error: {}",
        error.message
    );
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;
    assert_eq!(responses.requests().len(), 1);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_wire_idle_stream_reaches_the_turn() -> anyhow::Result<()> {
    let (gate_tx, gate_rx) = tokio::sync::oneshot::channel();
    let (server, _completions) = start_streaming_sse_server(vec![vec![
        StreamingSseChunk {
            gate: None,
            body: "data: {\"id\":\"chat_idle\",\"choices\":[]}\n\n".to_string(),
        },
        StreamingSseChunk {
            gate: Some(gate_rx),
            body: "data: [DONE]\n\n".to_string(),
        },
    ]])
    .await;
    let test = test_codex()
        .with_model("gpt-5.2")
        .with_config(|config| {
            configure_chat_provider(config);
            config.model_provider.stream_idle_timeout_ms = Some(20);
        })
        .build_with_streaming_server(&server)
        .await?;

    submit_text_turn(&test, "wait without progress").await?;
    let error = wait_for_event_match(&test.codex, |event| match event {
        EventMsg::Error(error) => Some(error.clone()),
        _ => None,
    })
    .await;
    assert_eq!(error.codex_error_info, Some(CodexErrorInfo::Other));
    assert!(
        error.message.contains("idle timeout"),
        "unexpected error: {}",
        error.message
    );
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    drop(gate_tx);
    server.shutdown().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_wire_cumulative_context_limit_reaches_the_turn() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let fragment = "x".repeat(6_000);
    let responses = mount_chat_sse_sequence(
        &server,
        vec![chat_sse(vec![
            json!({
                "id": "chat_context_limit",
                "choices": [{"index": 0, "delta": {"reasoning": fragment}}]
            }),
            json!({
                "choices": [{"index": 0, "delta": {"content": fragment}, "finish_reason": "stop"}]
            }),
        ])],
    )
    .await;
    let test = test_codex()
        .with_model("gpt-5.2")
        .with_config(configure_chat_provider)
        .build_with_auto_env(&server)
        .await?;

    submit_text_turn(&test, "produce too much context").await?;
    let error = wait_for_event_match(&test.codex, |event| match event {
        EventMsg::Error(error) => Some(error.clone()),
        _ => None,
    })
    .await;
    assert_eq!(error.codex_error_info, Some(CodexErrorInfo::Other));
    assert!(
        error.message.contains("response context limit"),
        "unexpected error: {}",
        error.message
    );
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;
    assert_eq!(responses.requests().len(), 1);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_wire_rejects_oversized_tool_set_before_http() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;
    let dynamic_tools = (0..72)
        .map(|index| {
            DynamicToolSpec::Function(DynamicToolFunctionSpec {
                name: format!("large_tool_{index}"),
                description: "x".repeat(3_600),
                input_schema: json!({
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false
                }),
                defer_loading: false,
            })
        })
        .collect();
    let test = test_codex()
        .with_model("gpt-5.2")
        .with_dynamic_tools(dynamic_tools)
        .with_config(configure_chat_provider)
        .build_with_auto_env(&server)
        .await?;

    submit_text_turn(&test, "use the oversized tool set").await?;
    let error = wait_for_event_match(&test.codex, |event| match event {
        EventMsg::Error(error) => Some(error.clone()),
        _ => None,
    })
    .await;
    assert!(
        error.message.contains("Chat tools exceed"),
        "unexpected error: {}",
        error.message
    );
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;
    server.verify().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_wire_reasoning_and_text_keep_item_lifecycles_sequential() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    mount_chat_sse_sequence(
        &server,
        vec![chat_sse(vec![
            json!({
                "id": "chatcmpl_reasoning",
                "choices": [{"index": 0, "delta": {"reasoning": "think"}}]
            }),
            json!({
                "id": "chatcmpl_reasoning",
                "choices": [{
                    "index": 0,
                    "delta": {"content": "answer"},
                    "finish_reason": "stop"
                }]
            }),
        ])],
    )
    .await;
    let test = test_codex()
        .with_model("gpt-5.2")
        .with_config(configure_chat_provider)
        .build_with_auto_env(&server)
        .await?;

    submit_text_turn(&test, "reason first").await?;
    let reasoning_started = wait_for_event_match(&test.codex, |event| match event {
        EventMsg::ItemStarted(ItemStartedEvent {
            item: item @ TurnItem::Reasoning(_),
            ..
        }) => Some(item.clone()),
        _ => None,
    })
    .await;
    let reasoning_completed = wait_for_event_match(&test.codex, |event| match event {
        EventMsg::ItemCompleted(ItemCompletedEvent {
            item: item @ TurnItem::Reasoning(_),
            ..
        }) => Some(item.clone()),
        _ => None,
    })
    .await;
    assert_eq!(reasoning_started.id(), reasoning_completed.id());

    let message_started = wait_for_event_match(&test.codex, |event| match event {
        EventMsg::ItemStarted(ItemStartedEvent {
            item: item @ TurnItem::AgentMessage(_),
            ..
        }) => Some(item.clone()),
        _ => None,
    })
    .await;
    let message_completed = wait_for_event_match(&test.codex, |event| match event {
        EventMsg::ItemCompleted(ItemCompletedEvent {
            item: item @ TurnItem::AgentMessage(_),
            ..
        }) => Some(item.clone()),
        _ => None,
    })
    .await;
    assert_eq!(message_started.id(), message_completed.id());
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;
    Ok(())
}

async fn submit_text_turn(test: &TestCodex, text: &str) -> anyhow::Result<()> {
    test.codex
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: text.to_string(),
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

fn configure_chat_provider(config: &mut Config) {
    config.model_provider.name = "Chat Completions".to_string();
    config.model_provider.env_key = None;
    config.model_provider.experimental_bearer_token = Some("test-token".to_string());
    config.model_provider.requires_openai_auth = false;
    config.model_provider.supports_websockets = true;
    config.model_provider.stream_max_retries = Some(0);
    config.model_provider.wire_api = WireApi::Chat;
}

fn chat_sse(events: Vec<Value>) -> String {
    let mut body = events
        .into_iter()
        .map(|event| format!("data: {event}\n\n"))
        .collect::<String>();
    body.push_str("data: [DONE]\n\n");
    body
}
