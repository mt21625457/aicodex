use anyhow::Result;
use codex_core::TurnInputRequest;
use codex_protocol::dynamic_tools::DynamicToolCallOutputContentItem;
use codex_protocol::dynamic_tools::DynamicToolFunctionSpec;
use codex_protocol::dynamic_tools::DynamicToolNamespaceSpec;
use codex_protocol::dynamic_tools::DynamicToolNamespaceTool;
use codex_protocol::dynamic_tools::DynamicToolResponse;
use codex_protocol::dynamic_tools::DynamicToolSpec;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::Op;
use codex_protocol::user_input::UserInput;
use core_test_support::apps_test_server::configure_search_capable_model;
use core_test_support::responses::ResponsesRequest;
use core_test_support::responses::ev_assistant_message;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_function_call_with_namespace;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::ev_tool_search_call;
use core_test_support::responses::mount_sse_sequence;
use core_test_support::responses::namespace_child_tool;
use core_test_support::responses::sse;
use core_test_support::responses::start_mock_server;
use core_test_support::responses::start_websocket_server;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::test_codex;
use core_test_support::wait_for_event;
use core_test_support::wait_for_successful_turn;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;
use test_case::test_case;

const SEARCH_ID: &str = "call_search_fixture";
const WINDOW_ID: &str = "call_window_fixture";
const FIXTURE_NAMESPACE: &str = "fixture_desktop";
const FIXTURE_TOOL: &str = "list_windows";

fn deferred_window_inventory() -> DynamicToolSpec {
    DynamicToolSpec::Namespace(DynamicToolNamespaceSpec {
        name: FIXTURE_NAMESPACE.to_string(),
        description: "Synthetic desktop inventory.".to_string(),
        tools: vec![DynamicToolNamespaceTool::Function(
            DynamicToolFunctionSpec {
                name: FIXTURE_TOOL.to_string(),
                description: "List windows in a synthetic desktop fixture.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false,
                }),
                defer_loading: true,
            },
        )],
    })
}

fn search_arguments() -> Value {
    json!({"query": FIXTURE_TOOL, "limit": 8})
}

fn aliased_search_event(call_id: &str, arguments: &str) -> Value {
    json!({
        "type": "response.output_item.done",
        "item": {
            "type": "function_call",
            "id": format!("fc_{call_id}"),
            "call_id": call_id,
            "status": "completed",
            "namespace": "tool_search",
            "name": "tool_search_tool",
            "arguments": arguments,
        },
    })
}

/// 续传与恢复都必须保留同一 call_id 的原生调用、实际工具结果及其顺序。
fn assert_canonical_search_pair(input: Vec<Value>) -> Vec<Value> {
    let items: Vec<_> = input
        .into_iter()
        .filter(|item| item["call_id"] == SEARCH_ID)
        .collect();
    assert_eq!(items.len(), 2, "search call and result must occur once");
    assert_eq!(items[0]["type"], "tool_search_call");
    assert_eq!(items[0]["execution"], "client");
    assert_eq!(items[0]["arguments"], search_arguments());
    assert_search_output(&items[1]);
    items
}

fn assert_search_output(item: &Value) {
    assert_eq!(item["type"], "tool_search_output");
    assert_eq!(item["execution"], "client");
    assert_eq!(item["call_id"], SEARCH_ID);
    assert!(namespace_child_tool(item, FIXTURE_NAMESPACE, FIXTURE_TOOL).is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tool_search_protocol_alias_survives_execution_and_restart() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    // 第一轮搜索，下一次推理调用已发现的工具；最后一次请求来自重启后的会话。
    let mock = mount_sse_sequence(
        &server,
        vec![
            sse(vec![
                ev_response_created("resp-search"),
                aliased_search_event(SEARCH_ID, &search_arguments().to_string()),
                ev_completed("resp-search"),
            ]),
            sse(vec![
                ev_response_created("resp-tool"),
                ev_function_call_with_namespace(WINDOW_ID, FIXTURE_NAMESPACE, FIXTURE_TOOL, "{}"),
                ev_completed("resp-tool"),
            ]),
            sse(vec![
                ev_response_created("resp-close"),
                ev_assistant_message("msg-close", "inventory read"),
                ev_completed("resp-close"),
            ]),
            sse(vec![
                ev_response_created("resp-resume"),
                ev_assistant_message("msg-resume", "history replayed"),
                ev_completed("resp-resume"),
            ]),
        ],
    )
    .await;
    let initial = test_codex()
        .with_config(configure_search_capable_model)
        .with_model_info_override("gpt-5.5", |model| model.use_responses_lite = true)
        .with_dynamic_tools(vec![deferred_window_inventory()])
        .build_with_auto_env(&server)
        .await?;
    initial
        .codex
        .start_or_steer_turn(TurnInputRequest::user_input(vec![UserInput::Text {
            text: "Find the synthetic list_windows tool and use it.".to_string(),
            text_elements: Vec::new(),
        }]))
        .await?;
    complete_window_inventory(&initial.codex).await?;
    wait_for_successful_turn(&initial.codex).await;
    let resumed = test_codex()
        .with_config(configure_search_capable_model)
        .with_model_info_override("gpt-5.5", |model| model.use_responses_lite = true)
        .restart_with_auto_env(&server, &initial)
        .await?;
    resumed
        .submit_text_turn("Continue the synthetic task.")
        .await?;

    let requests = mock.requests();
    assert_eq!(requests.len(), 4);
    assert!(requests[0].body_json()["tools"].is_null());
    let declarations = requests[0].inputs_of_type("additional_tools");
    assert!(declarations.iter().any(|declaration| {
        declaration["tools"].as_array().is_some_and(|tools| {
            tools
                .iter()
                .any(|tool| tool["type"] == "tool_search" && tool["execution"] == "client")
        })
    }));
    assert!(declarations.iter().all(|declaration| {
        namespace_child_tool(declaration, FIXTURE_NAMESPACE, FIXTURE_TOOL).is_none()
    }));
    for request in &requests[1..] {
        let pair = assert_canonical_search_pair(request.input());
        assert_eq!(pair[0]["status"], "completed");
        assert!(
            pair[0].get("id").is_none(),
            "canonical request must not reuse or invent a server response item ID"
        );
    }
    for request in &requests[2..] {
        assert_eq!(
            request.function_call_output_text(WINDOW_ID).as_deref(),
            Some("synthetic-window-ok")
        );
    }
    Ok(())
}

// 服务端缓存的原生调用可正常增量续传；普通函数缓存则必须重新发送原生调用和回执。
#[test_case(false; "native_search_continues_incrementally")]
#[test_case(true; "aliased_search_replays_canonical_pair")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tool_search_protocol_websocket_continuation(alias: bool) -> Result<()> {
    skip_if_no_network!(Ok(()));

    let arguments = search_arguments();
    let call = if alias {
        // 有的兼容服务不提供条目 ID 或状态，完成事件本身仍可承载有效调用。
        ev_function_call_with_namespace(
            SEARCH_ID,
            "tool_search",
            "tool_search_tool",
            &arguments.to_string(),
        )
    } else {
        ev_tool_search_call(SEARCH_ID, &arguments)
    };
    let server = start_websocket_server(vec![vec![
        vec![
            ev_response_created("resp-warmup"),
            ev_completed("resp-warmup"),
        ],
        vec![
            ev_response_created("resp-search"),
            call,
            ev_completed("resp-search"),
        ],
        vec![
            ev_response_created("resp-done"),
            ev_assistant_message("msg-done", "inventory found"),
            ev_completed("resp-done"),
        ],
    ]])
    .await;
    let test = test_codex()
        .with_config(configure_search_capable_model)
        .with_dynamic_tools(vec![deferred_window_inventory()])
        .build_with_websocket_server(&server)
        .await?;
    test.submit_text_turn("Find the synthetic window tool.")
        .await?;

    let connection = server.single_connection();
    assert_eq!(connection.len(), 3);
    let mut follow_up = connection[2].body_json();
    let Value::Array(input) = follow_up["input"].take() else {
        anyhow::bail!("expected input items in the WebSocket follow-up");
    };
    if alias {
        assert!(follow_up.get("previous_response_id").is_none());
        let pair = assert_canonical_search_pair(input);
        assert!(pair[0].get("id").is_none());
    } else {
        assert_eq!(follow_up["previous_response_id"], "resp-search");
        assert_eq!(input.len(), 1);
        assert_search_output(&input[0]);
    }
    server.shutdown().await;
    Ok(())
}

/// 回送客户端动态工具结果，确保下一步推理收到真实执行回执，而不是只验证名称解析。
async fn complete_window_inventory(thread: &codex_core::CodexThread) -> Result<()> {
    let event = wait_for_event(thread, |event| {
        matches!(
            event,
            EventMsg::DynamicToolCallRequest(_) | EventMsg::TurnComplete(_)
        )
    })
    .await;
    let EventMsg::DynamicToolCallRequest(request) = event else {
        anyhow::bail!("turn completed without reaching the discovered dynamic tool");
    };
    assert_eq!(request.call_id, WINDOW_ID);
    assert_eq!(request.namespace.as_deref(), Some(FIXTURE_NAMESPACE));
    assert_eq!(request.tool, FIXTURE_TOOL);
    assert_eq!(request.arguments, json!({}));
    thread
        .submit(Op::DynamicToolResponse {
            id: request.call_id,
            response: DynamicToolResponse {
                content_items: vec![DynamicToolCallOutputContentItem::InputText {
                    text: "synthetic-window-ok".to_string(),
                }],
                success: true,
            },
        })
        .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tool_search_protocol_keeps_native_calls_and_rejects_malformed_aliases() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let mock = mount_sse_sequence(
        &server,
        vec![
            sse(vec![
                ev_response_created("resp-native"),
                ev_tool_search_call(SEARCH_ID, &search_arguments()),
                ev_function_call_with_namespace(
                    "call_wrong_name",
                    "tool_search",
                    "lookalike",
                    &search_arguments().to_string(),
                ),
                ev_function_call_with_namespace(
                    "call_wrong_namespace",
                    "other_namespace",
                    "tool_search_tool",
                    &search_arguments().to_string(),
                ),
                aliased_search_event("call_invalid_json", "{broken"),
                aliased_search_event("call_invalid_params", r#"{"query":42}"#),
                aliased_search_event("call_array_params", r#"["list_windows",8]"#),
                {
                    let mut item =
                        aliased_search_event("call_incomplete", &search_arguments().to_string());
                    item["item"]["status"] = json!("incomplete");
                    item
                },
                ev_completed("resp-native"),
            ]),
            sse(vec![
                ev_response_created("resp-done"),
                ev_assistant_message("msg-done", "done"),
                ev_completed("resp-done"),
            ]),
        ],
    )
    .await;
    let test = test_codex()
        .with_config(configure_search_capable_model)
        .with_dynamic_tools(vec![deferred_window_inventory()])
        .build_with_auto_env(&server)
        .await?;
    test.submit_text_turn("Find the synthetic tool.").await?;
    let requests = mock.requests();
    assert_eq!(requests.len(), 2);
    let follow_up = &requests[1];
    assert_canonical_search_pair(follow_up.input());
    assert_unsupported_function(follow_up, "call_wrong_name", "tool_searchlookalike");
    assert_unsupported_function(
        follow_up,
        "call_wrong_namespace",
        "other_namespacetool_search_tool",
    );
    assert_unsupported_function(
        follow_up,
        "call_invalid_json",
        "tool_searchtool_search_tool",
    );
    assert_unsupported_function(
        follow_up,
        "call_invalid_params",
        "tool_searchtool_search_tool",
    );
    assert_unsupported_function(
        follow_up,
        "call_array_params",
        "tool_searchtool_search_tool",
    );
    assert_unsupported_function(follow_up, "call_incomplete", "tool_searchtool_search_tool");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tool_search_protocol_does_not_admit_alias_without_advertised_search() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let mock = mount_sse_sequence(
        &server,
        vec![
            sse(vec![
                ev_response_created("resp-unavailable"),
                ev_function_call_with_namespace(
                    SEARCH_ID,
                    "tool_search",
                    "tool_search_tool",
                    &search_arguments().to_string(),
                ),
                ev_completed("resp-unavailable"),
            ]),
            sse(vec![
                ev_response_created("resp-done"),
                ev_assistant_message("msg-done", "done"),
                ev_completed("resp-done"),
            ]),
        ],
    )
    .await;
    // 模型未声明原生搜索，即使运行时有延迟工具，也不能执行搜索别名。
    let test = test_codex()
        .with_config(configure_search_capable_model)
        .with_model_info_override("gpt-5.5", |model| model.supports_search_tool = false)
        .with_dynamic_tools(vec![deferred_window_inventory()])
        .build_with_auto_env(&server)
        .await?;
    test.submit_text_turn("Search for a tool that is not registered.")
        .await?;
    let requests = mock.requests();
    assert_eq!(requests.len(), 2);
    let first = requests[0].body_json();
    assert!(
        first["tools"]
            .as_array()
            .is_some_and(|tools| tools.iter().all(|tool| tool["type"] != "tool_search"))
    );
    assert_unsupported_function(&requests[1], SEARCH_ID, "tool_searchtool_search_tool");
    Ok(())
}

fn assert_unsupported_function(request: &ResponsesRequest, call_id: &str, formatted_name: &str) {
    assert!(request.has_function_call(call_id));
    assert_eq!(
        request.function_call_output_text(call_id),
        Some(format!("unsupported call: {formatted_name}"))
    );
    assert!(
        request
            .inputs_of_type("tool_search_output")
            .iter()
            .all(|item| item["call_id"] != call_id)
    );
}
