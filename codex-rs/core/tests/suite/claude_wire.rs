use codex_core::config::Config;
use codex_model_provider_info::WireApi;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::Op;
use codex_protocol::user_input::UserInput;
use core_test_support::responses::mount_claude_sse_sequence;
use core_test_support::responses::sse;
use core_test_support::responses::start_mock_server;
use core_test_support::test_codex::test_codex;
use core_test_support::wait_for_event;
use core_test_support::wait_for_event_match;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;

const EXISTING_ENV_VAR_WITH_NON_EMPTY_VALUE: &str = "PATH";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn claude_wire_tool_loop_posts_messages_and_tool_result() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let responses = mount_claude_sse_sequence(
        &server,
        vec![claude_tool_use_sse(), claude_text_sse("msg_2", "done")],
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
