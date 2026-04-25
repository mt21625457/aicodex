use codex_core::config::Config;
use codex_model_provider_info::WireApi;
use core_test_support::responses::mount_claude_sse_sequence;
use core_test_support::responses::sse;
use core_test_support::responses::start_mock_server;
use core_test_support::test_codex::test_codex;
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

fn configure_claude_provider(config: &mut Config) {
    config.model_provider.name = "Anthropic".to_string();
    config.model_provider.env_key = Some(EXISTING_ENV_VAR_WITH_NON_EMPTY_VALUE.to_string());
    config.model_provider.experimental_bearer_token = None;
    config.model_provider.requires_openai_auth = false;
    config.model_provider.supports_websockets = false;
    config.model_provider.wire_api = WireApi::Claude;
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
