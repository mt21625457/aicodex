use crate::dto::AnthropicTurnRequest;
use crate::request::build_request;
use crate::request::encode_anthropic_reasoning_blocks;
use crate::stream_anthropic;
use codex_api::ResponseEvent;
use codex_model_provider_info::ModelProviderInfo;
use codex_model_provider_info::WireApi;
use codex_protocol::config_types::ReasoningSummary;
use codex_protocol::config_types::ServiceTier;
use codex_protocol::models::BaseInstructions;
use codex_protocol::models::ContentItem;
use codex_protocol::models::FunctionCallOutputContentItem;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::ReasoningItemContent;
use codex_protocol::models::ReasoningItemReasoningSummary;
use codex_protocol::models::ResponseItem;
use codex_protocol::openai_models::ReasoningEffort;
use pretty_assertions::assert_eq;
use serde_json::json;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::Request;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path;

fn test_provider(base_url: String) -> ModelProviderInfo {
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

fn test_request(base_url: String) -> AnthropicTurnRequest {
    AnthropicTurnRequest {
        provider: test_provider(base_url),
        model: "claude-test".to_string(),
        input: vec![
            ResponseItem::Message {
                id: None,
                role: "developer".to_string(),
                content: vec![ContentItem::InputText {
                    text: "developer rules".to_string(),
                }],
                end_turn: None,
                phase: None,
            },
            ResponseItem::Message {
                id: None,
                role: "user".to_string(),
                content: vec![ContentItem::InputText {
                    text: "hello".to_string(),
                }],
                end_turn: None,
                phase: None,
            },
        ],
        tools: Vec::new(),
        parallel_tool_calls: false,
        base_instructions: BaseInstructions {
            text: "base".to_string(),
        },
        effort: None,
        summary: ReasoningSummary::Auto,
        service_tier: None,
        turn_metadata_header: None,
        output_schema: None,
    }
}

#[tokio::test]
async fn build_request_maps_system_and_messages() {
    let request = build_request(&test_request("https://api.anthropic.com".to_string()))
        .expect("request should build");
    assert_eq!(request.model, "claude-test");
    assert_eq!(request.messages.len(), 1);
    assert_eq!(request.messages[0].role, "user");
    assert_eq!(
        request.extra.get("system"),
        Some(&json!("base\n\ndeveloper rules"))
    );
}

#[tokio::test]
async fn build_request_includes_function_tool_schema() {
    let mut request = test_request("https://api.anthropic.com".to_string());
    request.tools.push(codex_tools::ToolSpec::Function(
        codex_tools::ResponsesApiTool {
            name: "time".to_string(),
            description: "Returns current time".to_string(),
            strict: false,
            defer_loading: None,
            parameters: codex_tools::JsonSchema::object(
                std::collections::BTreeMap::from([(
                    "utc_offset".to_string(),
                    codex_tools::JsonSchema::string(None),
                )]),
                Some(vec!["utc_offset".to_string()]),
                Some(codex_tools::AdditionalProperties::Boolean(false)),
            ),
            output_schema: None,
        },
    ));

    let built = build_request(&request).expect("request should build");
    let tools = built.extra.get("tools").expect("tools should be present");
    assert_eq!(tools[0]["name"], json!("time"));
    assert_eq!(tools[0]["input_schema"]["type"], json!("object"));
}

#[tokio::test]
async fn build_request_includes_freeform_tool_schema() {
    let mut request = test_request("https://api.anthropic.com".to_string());
    request
        .tools
        .push(codex_tools::ToolSpec::Freeform(codex_tools::FreeformTool {
            name: "apply_patch".to_string(),
            description: "Patch files".to_string(),
            format: codex_tools::FreeformToolFormat {
                r#type: "grammar".to_string(),
                syntax: "lark".to_string(),
                definition: "dummy".to_string(),
            },
        }));

    let built = build_request(&request).expect("request should build");
    let tools = built.extra.get("tools").expect("tools should be present");
    assert_eq!(tools[0]["name"], json!("apply_patch"));
    assert_eq!(
        tools[0]["input_schema"]["properties"]["input"]["type"],
        json!("string")
    );
}

#[tokio::test]
async fn build_request_supports_image_inputs() {
    let mut request = test_request("https://api.anthropic.com".to_string());
    request.input = vec![ResponseItem::Message {
        id: None,
        role: "user".to_string(),
        content: vec![
            ContentItem::InputImage {
                image_url: "data:image/png;base64,QUJD".to_string(),
            },
            ContentItem::InputText {
                text: "describe this".to_string(),
            },
        ],
        end_turn: None,
        phase: None,
    }];

    let built = build_request(&request).expect("request should build");
    let serialized = serde_json::to_value(&built.messages[0]).expect("serialize message");
    assert_eq!(serialized["role"], json!("user"));
    assert_eq!(serialized["content"][0]["type"], json!("image"));
    assert_eq!(
        serialized["content"][0]["source"]["media_type"],
        json!("image/png")
    );
    assert_eq!(serialized["content"][1]["text"], json!("describe this"));
}

#[tokio::test]
async fn build_request_preserves_structured_tool_results() {
    let mut request = test_request("https://api.anthropic.com".to_string());
    request.input.push(ResponseItem::FunctionCallOutput {
        call_id: "call_view_1".to_string(),
        output: FunctionCallOutputPayload::from_content_items(vec![
            FunctionCallOutputContentItem::InputText {
                text: "tool text".to_string(),
            },
            FunctionCallOutputContentItem::InputImage {
                image_url: "data:image/png;base64,QUJD".to_string(),
                detail: None,
            },
        ]),
    });

    let built = build_request(&request).expect("request should build");
    let serialized = serde_json::to_value(&built.messages[1]).expect("serialize tool result");
    assert_eq!(serialized["role"], json!("user"));
    assert_eq!(serialized["content"][0]["type"], json!("tool_result"));
    assert_eq!(
        serialized["content"][0]["content"][0]["text"],
        json!("tool text")
    );
    assert_eq!(
        serialized["content"][0]["content"][1]["source"]["media_type"],
        json!("image/png")
    );
}

#[tokio::test]
async fn build_request_includes_thinking_controls_when_effort_is_enabled() {
    let mut request = test_request("https://api.anthropic.com".to_string());
    request.effort = Some(ReasoningEffort::High);
    request.summary = ReasoningSummary::Detailed;

    let built = build_request(&request).expect("request should build");
    assert_eq!(built.max_tokens, 12_288);
    assert_eq!(
        built.extra.get("thinking"),
        Some(&json!({
            "type": "enabled",
            "budget_tokens": 8192,
            "display": "summarized"
        }))
    );
}

#[tokio::test]
async fn build_request_includes_service_tier_when_set() {
    let mut request = test_request("https://api.anthropic.com".to_string());
    request.service_tier = Some(ServiceTier::Fast);

    let built = build_request(&request).expect("request should build");
    assert_eq!(built.extra.get("service_tier"), Some(&json!("auto")));
}

#[tokio::test]
async fn build_request_includes_tool_search_schema() {
    let mut request = test_request("https://api.anthropic.com".to_string());
    request.tools.push(codex_tools::ToolSpec::ToolSearch {
        execution: "client".to_string(),
        description: "Search for tools".to_string(),
        parameters: codex_tools::JsonSchema::object(
            std::collections::BTreeMap::from([
                ("query".to_string(), codex_tools::JsonSchema::string(None)),
                ("limit".to_string(), codex_tools::JsonSchema::number(None)),
            ]),
            Some(vec!["query".to_string()]),
            Some(codex_tools::AdditionalProperties::Boolean(false)),
        ),
    });

    let built = build_request(&request).expect("request should build");
    let tools = built.extra.get("tools").expect("tools should be present");
    assert_eq!(tools[0]["name"], json!("tool_search"));
    assert_eq!(
        tools[0]["input_schema"]["properties"]["query"]["type"],
        json!("string")
    );
}

#[tokio::test]
async fn build_request_includes_web_search_server_tool() {
    let mut request = test_request("https://api.anthropic.com".to_string());
    request.tools.push(codex_tools::ToolSpec::WebSearch {
        external_web_access: Some(true),
        filters: Some(codex_tools::ResponsesApiWebSearchFilters {
            allowed_domains: Some(vec!["example.com".to_string()]),
        }),
        user_location: None,
        search_context_size: None,
        search_content_types: None,
    });

    let built = build_request(&request).expect("request should build");
    let tools = built.extra.get("tools").expect("tools should be present");
    assert_eq!(tools[0]["type"], json!("web_search_20250305"));
    assert_eq!(tools[0]["name"], json!("web_search"));
    assert_eq!(tools[0]["allowed_domains"], json!(["example.com"]));
}

#[tokio::test]
async fn build_request_includes_image_generation_server_tool_and_replays_history() {
    let mut request = test_request("https://api.anthropic.com".to_string());
    request.tools.push(codex_tools::ToolSpec::ImageGeneration {
        output_format: "png".to_string(),
    });
    request.input.push(ResponseItem::ImageGenerationCall {
        id: "ig_hist_1".to_string(),
        status: "completed".to_string(),
        revised_prompt: Some("a red kite".to_string()),
        result: "QUJD".to_string(),
    });

    let built = build_request(&request).expect("request should build");
    let tools = built.extra.get("tools").expect("tools should be present");
    assert_eq!(tools[0]["type"], json!("image_generation"));
    assert_eq!(tools[0]["name"], json!("image_generation"));
    assert_eq!(tools[0]["output_format"], json!("png"));

    let serialized = serde_json::to_value(&built.messages[1]).expect("serialize assistant history");
    assert_eq!(serialized["role"], json!("assistant"));
    assert_eq!(serialized["content"][0]["type"], json!("server_tool_use"));
    assert_eq!(serialized["content"][0]["name"], json!("image_generation"));
    assert_eq!(
        serialized["content"][0]["input"]["prompt"],
        json!("a red kite")
    );
    assert_eq!(
        serialized["content"][1]["type"],
        json!("image_generation_tool_result")
    );
    assert_eq!(serialized["content"][1]["tool_use_id"], json!("ig_hist_1"));
    assert_eq!(serialized["content"][1]["result"], json!("QUJD"));
    assert_eq!(
        serialized["content"][1]["content"][0]["source"]["media_type"],
        json!("image/png")
    );
}

#[tokio::test]
async fn build_request_round_trips_anthropic_reasoning_blocks() {
    let mut request = test_request("https://api.anthropic.com".to_string());
    request.input = vec![
        ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: "hello".to_string(),
            }],
            end_turn: None,
            phase: None,
        },
        ResponseItem::Reasoning {
            id: "reasoning-1".to_string(),
            summary: vec![ReasoningItemReasoningSummary::SummaryText {
                text: "step one".to_string(),
            }],
            content: Some(vec![ReasoningItemContent::ReasoningText {
                text: "step one".to_string(),
            }]),
            encrypted_content: encode_anthropic_reasoning_blocks(&[json!({
                "type": "thinking",
                "thinking": "step one",
                "signature": "sig-1"
            })]),
        },
        ResponseItem::FunctionCall {
            id: None,
            name: "time".to_string(),
            namespace: None,
            arguments: json!({"utc_offset":"+00:00"}).to_string(),
            call_id: "call_time_1".to_string(),
        },
    ];

    let built = build_request(&request).expect("request should build");
    let serialized = serde_json::to_value(&built.messages[1]).expect("serialize assistant message");
    assert_eq!(serialized["role"], json!("assistant"));
    assert_eq!(serialized["content"][0]["type"], json!("thinking"));
    assert_eq!(serialized["content"][0]["signature"], json!("sig-1"));
    assert_eq!(serialized["content"][1]["type"], json!("tool_use"));
}

#[tokio::test]
async fn stream_anthropic_streams_text_response() {
    let server = MockServer::start().await;
    let sse = [
        "event: message_start",
        r#"data: {"type":"message_start","message":{"id":"resp-1","content":[],"model":"claude-test","role":"assistant","type":"message","usage":{}}}"#,
        "",
        "event: content_block_delta",
        r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hello"}}"#,
        "",
        "event: message_delta",
        r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"input_tokens":4,"output_tokens":1}}"#,
        "",
        "event: message_stop",
        r#"data: {"type":"message_stop"}"#,
        "",
    ]
    .join("\n");

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(sse, "text/event-stream"),
        )
        .mount(&server)
        .await;

    let request = test_request(server.uri());
    let mut stream = stream_anthropic(request).await.expect("stream receiver");
    let mut saw_delta = false;
    let mut saw_done = false;
    let mut saw_completed = false;

    while let Some(event) = stream.recv().await {
        match event.expect("response event") {
            ResponseEvent::OutputTextDelta(delta) => {
                saw_delta = true;
                assert_eq!(delta, "hello");
            }
            ResponseEvent::OutputItemDone(ResponseItem::Message { content, .. }) => {
                saw_done = true;
                assert_eq!(
                    content,
                    vec![ContentItem::OutputText {
                        text: "hello".to_string()
                    }]
                );
            }
            ResponseEvent::Completed { .. } => {
                saw_completed = true;
                break;
            }
            _ => {}
        }
    }

    assert!(saw_delta);
    assert!(saw_done);
    assert!(saw_completed);
}

#[tokio::test]
async fn stream_anthropic_streams_reasoning_response() {
    let server = MockServer::start().await;
    let sse = [
        "event: message_start",
        r#"data: {"type":"message_start","message":{"id":"resp-2","content":[],"model":"claude-test","role":"assistant","type":"message","usage":{}}}"#,
        "",
        "event: content_block_start",
        r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":""}}"#,
        "",
        "event: content_block_delta",
        r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"step one"}}"#,
        "",
        "event: content_block_delta",
        r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig-1"}}"#,
        "",
        "event: content_block_stop",
        r#"data: {"type":"content_block_stop","index":0}"#,
        "",
        "event: content_block_delta",
        r#"data: {"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"done"}}"#,
        "",
        "event: message_delta",
        r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"input_tokens":4,"output_tokens":2}}"#,
        "",
        "event: message_stop",
        r#"data: {"type":"message_stop"}"#,
        "",
    ]
    .join("\n");

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(sse, "text/event-stream"),
        )
        .mount(&server)
        .await;

    let request = test_request(server.uri());
    let mut stream = stream_anthropic(request).await.expect("stream receiver");
    let mut saw_reasoning_added = false;
    let mut saw_reasoning_delta = false;
    let mut saw_reasoning_done = false;

    while let Some(event) = stream.recv().await {
        match event.expect("response event") {
            ResponseEvent::OutputItemAdded(ResponseItem::Reasoning { .. }) => {
                saw_reasoning_added = true;
            }
            ResponseEvent::ReasoningSummaryDelta { delta, .. } => {
                saw_reasoning_delta = true;
                assert_eq!(delta, "step one");
            }
            ResponseEvent::OutputItemDone(ResponseItem::Reasoning {
                summary,
                content,
                encrypted_content,
                ..
            }) => {
                saw_reasoning_done = true;
                assert_eq!(
                    summary,
                    vec![
                        codex_protocol::models::ReasoningItemReasoningSummary::SummaryText {
                            text: "step one".to_string()
                        }
                    ]
                );
                assert_eq!(
                    content,
                    Some(vec![
                        codex_protocol::models::ReasoningItemContent::ReasoningText {
                            text: "step one".to_string()
                        }
                    ])
                );
                assert_eq!(
                    encrypted_content,
                    encode_anthropic_reasoning_blocks(&[json!({
                        "type": "thinking",
                        "thinking": "step one",
                        "signature": "sig-1"
                    })])
                );
            }
            ResponseEvent::Completed { .. } => break,
            _ => {}
        }
    }

    assert!(saw_reasoning_added);
    assert!(saw_reasoning_delta);
    assert!(saw_reasoning_done);
}

#[tokio::test]
async fn stream_anthropic_streams_tool_use_response() {
    let server = MockServer::start().await;
    let sse = [
        "event: message_start",
        r#"data: {"type":"message_start","message":{"id":"resp-tool-1","content":[],"model":"claude-test","role":"assistant","type":"message","usage":{}}}"#,
        "",
        "event: content_block_start",
        r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"call_time_1","name":"time","input":{"utc_offset":"+00:00"}}}"#,
        "",
        "event: content_block_stop",
        r#"data: {"type":"content_block_stop","index":0}"#,
        "",
        "event: message_delta",
        r#"data: {"type":"message_delta","delta":{"stop_reason":"tool_use","stop_sequence":null},"usage":{"input_tokens":4,"output_tokens":1}}"#,
        "",
        "event: message_stop",
        r#"data: {"type":"message_stop"}"#,
        "",
    ]
    .join("\n");

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(sse, "text/event-stream"),
        )
        .mount(&server)
        .await;

    let mut request = test_request(server.uri());
    request.tools.push(codex_tools::ToolSpec::Function(
        codex_tools::ResponsesApiTool {
            name: "time".to_string(),
            description: "Returns current time".to_string(),
            strict: false,
            defer_loading: None,
            parameters: codex_tools::JsonSchema::object(
                std::collections::BTreeMap::from([(
                    "utc_offset".to_string(),
                    codex_tools::JsonSchema::string(None),
                )]),
                Some(vec!["utc_offset".to_string()]),
                Some(codex_tools::AdditionalProperties::Boolean(false)),
            ),
            output_schema: None,
        },
    ));

    let mut stream = stream_anthropic(request).await.expect("stream receiver");
    let mut saw_tool = false;
    while let Some(event) = stream.recv().await {
        match event.expect("response event") {
            ResponseEvent::OutputItemDone(ResponseItem::FunctionCall {
                name,
                call_id,
                arguments,
                ..
            }) => {
                saw_tool = true;
                assert_eq!(name, "time");
                assert_eq!(call_id, "call_time_1");
                assert_eq!(arguments, json!({"utc_offset":"+00:00"}).to_string());
            }
            ResponseEvent::Completed { .. } => break,
            _ => {}
        }
    }

    assert!(saw_tool);
}

#[tokio::test]
async fn stream_anthropic_streams_custom_tool_use_response() {
    let server = MockServer::start().await;
    let sse = [
        "event: message_start",
        r#"data: {"type":"message_start","message":{"id":"resp-tool-3","content":[],"model":"claude-test","role":"assistant","type":"message","usage":{}}}"#,
        "",
        "event: content_block_start",
        r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"call_patch_1","name":"apply_patch","input":{"input":"*** Begin Patch\n*** End Patch\n"}}}"#,
        "",
        "event: content_block_stop",
        r#"data: {"type":"content_block_stop","index":0}"#,
        "",
        "event: message_delta",
        r#"data: {"type":"message_delta","delta":{"stop_reason":"tool_use","stop_sequence":null},"usage":{"input_tokens":4,"output_tokens":1}}"#,
        "",
        "event: message_stop",
        r#"data: {"type":"message_stop"}"#,
        "",
    ]
    .join("\n");

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(sse, "text/event-stream"),
        )
        .mount(&server)
        .await;

    let mut request = test_request(server.uri());
    request
        .tools
        .push(codex_tools::ToolSpec::Freeform(codex_tools::FreeformTool {
            name: "apply_patch".to_string(),
            description: "Patch files".to_string(),
            format: codex_tools::FreeformToolFormat {
                r#type: "grammar".to_string(),
                syntax: "lark".to_string(),
                definition: "dummy".to_string(),
            },
        }));

    let mut stream = stream_anthropic(request).await.expect("stream receiver");
    let mut saw_tool = false;
    while let Some(event) = stream.recv().await {
        match event.expect("response event") {
            ResponseEvent::OutputItemDone(ResponseItem::CustomToolCall {
                name,
                call_id,
                input,
                ..
            }) => {
                saw_tool = true;
                assert_eq!(name, "apply_patch");
                assert_eq!(call_id, "call_patch_1");
                assert_eq!(input, "*** Begin Patch\n*** End Patch\n");
            }
            ResponseEvent::Completed { .. } => break,
            _ => {}
        }
    }

    assert!(saw_tool);
}

#[tokio::test]
async fn stream_anthropic_streams_local_shell_tool_use_response() {
    let server = MockServer::start().await;
    let sse = [
        "event: message_start",
        r#"data: {"type":"message_start","message":{"id":"resp-tool-4","content":[],"model":"claude-test","role":"assistant","type":"message","usage":{}}}"#,
        "",
        "event: content_block_start",
        r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"call_shell_1","name":"local_shell","input":{"command":["pwd"],"workdir":"/tmp","timeout_ms":1234}}}"#,
        "",
        "event: content_block_stop",
        r#"data: {"type":"content_block_stop","index":0}"#,
        "",
        "event: message_delta",
        r#"data: {"type":"message_delta","delta":{"stop_reason":"tool_use","stop_sequence":null},"usage":{"input_tokens":4,"output_tokens":1}}"#,
        "",
        "event: message_stop",
        r#"data: {"type":"message_stop"}"#,
        "",
    ]
    .join("\n");

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(sse, "text/event-stream"),
        )
        .mount(&server)
        .await;

    let mut request = test_request(server.uri());
    request.tools.push(codex_tools::ToolSpec::LocalShell {});

    let mut stream = stream_anthropic(request).await.expect("stream receiver");
    let mut saw_tool = false;
    while let Some(event) = stream.recv().await {
        match event.expect("response event") {
            ResponseEvent::OutputItemDone(ResponseItem::LocalShellCall {
                call_id,
                action,
                status,
                ..
            }) => {
                saw_tool = true;
                assert_eq!(call_id, Some("call_shell_1".to_string()));
                assert_eq!(status, codex_protocol::models::LocalShellStatus::InProgress);
                let codex_protocol::models::LocalShellAction::Exec(exec) = action;
                assert_eq!(exec.command, vec!["pwd".to_string()]);
                assert_eq!(exec.working_directory, Some("/tmp".to_string()));
                assert_eq!(exec.timeout_ms, Some(1234));
            }
            ResponseEvent::Completed { .. } => break,
            _ => {}
        }
    }

    assert!(saw_tool);
}

#[tokio::test]
async fn stream_anthropic_streams_tool_search_response() {
    let server = MockServer::start().await;
    let sse = [
        "event: message_start",
        r#"data: {"type":"message_start","message":{"id":"resp-tool-search-1","content":[],"model":"claude-test","role":"assistant","type":"message","usage":{}}}"#,
        "",
        "event: content_block_start",
        r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"call_tool_search_1","name":"tool_search","input":{"query":"notion","limit":3}}}"#,
        "",
        "event: content_block_stop",
        r#"data: {"type":"content_block_stop","index":0}"#,
        "",
        "event: message_delta",
        r#"data: {"type":"message_delta","delta":{"stop_reason":"tool_use","stop_sequence":null},"usage":{"input_tokens":4,"output_tokens":1}}"#,
        "",
        "event: message_stop",
        r#"data: {"type":"message_stop"}"#,
        "",
    ]
    .join("\n");

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(sse, "text/event-stream"),
        )
        .mount(&server)
        .await;

    let mut request = test_request(server.uri());
    request.tools.push(codex_tools::ToolSpec::ToolSearch {
        execution: "client".to_string(),
        description: "Search for tools".to_string(),
        parameters: codex_tools::JsonSchema::object(
            std::collections::BTreeMap::from([
                ("query".to_string(), codex_tools::JsonSchema::string(None)),
                ("limit".to_string(), codex_tools::JsonSchema::number(None)),
            ]),
            Some(vec!["query".to_string()]),
            Some(codex_tools::AdditionalProperties::Boolean(false)),
        ),
    });

    let mut stream = stream_anthropic(request).await.expect("stream receiver");
    let mut saw_tool = false;
    while let Some(event) = stream.recv().await {
        match event.expect("response event") {
            ResponseEvent::OutputItemDone(ResponseItem::ToolSearchCall {
                call_id,
                execution,
                arguments,
                ..
            }) => {
                saw_tool = true;
                assert_eq!(call_id, Some("call_tool_search_1".to_string()));
                assert_eq!(execution, "client");
                assert_eq!(arguments, json!({"query":"notion","limit":3}));
            }
            ResponseEvent::Completed { .. } => break,
            _ => {}
        }
    }

    assert!(saw_tool);
}

#[tokio::test]
async fn stream_anthropic_streams_web_search_server_tool_response() {
    let server = MockServer::start().await;
    let sse = [
        "event: message_start",
        r#"data: {"type":"message_start","message":{"id":"resp-web-search-1","content":[],"model":"claude-test","role":"assistant","type":"message","usage":{}}}"#,
        "",
        "event: content_block_start",
        r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"server_tool_use","id":"srvtoolu_ws_1","name":"web_search","input":{"query":"rust ratatui"}}}"#,
        "",
        "event: content_block_stop",
        r#"data: {"type":"content_block_stop","index":0}"#,
        "",
        "event: content_block_start",
        r#"data: {"type":"content_block_start","index":1,"content_block":{"type":"web_search_tool_result","tool_use_id":"srvtoolu_ws_1","content":[]}}"#,
        "",
        "event: content_block_stop",
        r#"data: {"type":"content_block_stop","index":1}"#,
        "",
        "event: content_block_delta",
        r#"data: {"type":"content_block_delta","index":2,"delta":{"type":"text_delta","text":"done"}}"#,
        "",
        "event: message_delta",
        r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"input_tokens":4,"output_tokens":2}}"#,
        "",
        "event: message_stop",
        r#"data: {"type":"message_stop"}"#,
        "",
    ]
    .join("\n");

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(sse, "text/event-stream"),
        )
        .mount(&server)
        .await;

    let mut request = test_request(server.uri());
    request.tools.push(codex_tools::ToolSpec::WebSearch {
        external_web_access: Some(true),
        filters: None,
        user_location: None,
        search_context_size: None,
        search_content_types: None,
    });

    let mut stream = stream_anthropic(request).await.expect("stream receiver");
    let mut saw_web_search = false;
    while let Some(event) = stream.recv().await {
        match event.expect("response event") {
            ResponseEvent::OutputItemDone(ResponseItem::WebSearchCall { id, status, action }) => {
                saw_web_search = true;
                assert_eq!(id, Some("srvtoolu_ws_1".to_string()));
                assert_eq!(status, Some("completed".to_string()));
                assert_eq!(
                    action,
                    Some(codex_protocol::models::WebSearchAction::Search {
                        query: Some("rust ratatui".to_string()),
                        queries: None,
                    })
                );
            }
            ResponseEvent::Completed { .. } => break,
            _ => {}
        }
    }

    assert!(saw_web_search);
}

#[tokio::test]
async fn stream_anthropic_streams_image_generation_server_tool_response() {
    let server = MockServer::start().await;
    let sse = [
        "event: message_start",
        r#"data: {"type":"message_start","message":{"id":"resp-image-gen-1","content":[],"model":"claude-test","role":"assistant","type":"message","usage":{}}}"#,
        "",
        "event: content_block_start",
        r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"server_tool_use","id":"srvtoolu_img_1","name":"image_generation","input":{"prompt":"a red kite"}}}"#,
        "",
        "event: content_block_stop",
        r#"data: {"type":"content_block_stop","index":0}"#,
        "",
        "event: content_block_start",
        r#"data: {"type":"content_block_start","index":1,"content_block":{"type":"image_generation_tool_result","tool_use_id":"srvtoolu_img_1","revised_prompt":"a bright red kite over water","result":"QUJD","content":[{"type":"image","source":{"type":"base64","media_type":"image/png","data":"QUJD"}}]}}"#,
        "",
        "event: content_block_stop",
        r#"data: {"type":"content_block_stop","index":1}"#,
        "",
        "event: message_delta",
        r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"input_tokens":4,"output_tokens":2}}"#,
        "",
        "event: message_stop",
        r#"data: {"type":"message_stop"}"#,
        "",
    ]
    .join("\n");

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(sse, "text/event-stream"),
        )
        .mount(&server)
        .await;

    let mut request = test_request(server.uri());
    request.tools.push(codex_tools::ToolSpec::ImageGeneration {
        output_format: "png".to_string(),
    });

    let mut stream = stream_anthropic(request).await.expect("stream receiver");
    let mut saw_image_generation = false;
    while let Some(event) = stream.recv().await {
        match event.expect("response event") {
            ResponseEvent::OutputItemDone(ResponseItem::ImageGenerationCall {
                id,
                status,
                revised_prompt,
                result,
            }) => {
                saw_image_generation = true;
                assert_eq!(id, "srvtoolu_img_1");
                assert_eq!(status, "completed");
                assert_eq!(
                    revised_prompt,
                    Some("a bright red kite over water".to_string())
                );
                assert_eq!(result, "QUJD");
            }
            ResponseEvent::Completed { .. } => break,
            _ => {}
        }
    }

    assert!(saw_image_generation);
}

#[tokio::test]
async fn stream_anthropic_prefers_api_key_over_bearer_auth() {
    let server = MockServer::start().await;
    let sse = [
        "event: message_start",
        r#"data: {"type":"message_start","message":{"id":"resp-auth-1","content":[],"model":"claude-test","role":"assistant","type":"message","usage":{}}}"#,
        "",
        "event: content_block_delta",
        r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"ok"}}"#,
        "",
        "event: message_delta",
        r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"input_tokens":4,"output_tokens":1}}"#,
        "",
        "event: message_stop",
        r#"data: {"type":"message_stop"}"#,
        "",
    ]
    .join("\n");

    let auth_matcher = |req: &Request| {
        req.headers.get("x-api-key").is_some() && req.headers.get("authorization").is_none()
    };
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(auth_matcher)
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(sse, "text/event-stream"),
        )
        .mount(&server)
        .await;

    let request = AnthropicTurnRequest {
        provider: ModelProviderInfo {
            name: "Anthropic".to_string(),
            base_url: Some(server.uri()),
            env_key: Some("PATH".to_string()),
            env_key_instructions: None,
            experimental_bearer_token: Some("bearer-token".to_string()),
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
        },
        model: "claude-test".to_string(),
        input: vec![ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: "hello".to_string(),
            }],
            end_turn: None,
            phase: None,
        }],
        tools: Vec::new(),
        parallel_tool_calls: false,
        base_instructions: BaseInstructions {
            text: "base".to_string(),
        },
        effort: None,
        summary: ReasoningSummary::Auto,
        service_tier: None,
        turn_metadata_header: None,
        output_schema: None,
    };

    let mut stream = stream_anthropic(request).await.expect("stream receiver");
    while let Some(event) = stream.recv().await {
        if matches!(
            event.expect("response event"),
            ResponseEvent::Completed { .. }
        ) {
            break;
        }
    }
}

#[tokio::test]
async fn stream_anthropic_includes_turn_metadata_header_when_present() {
    let server = MockServer::start().await;
    let sse = [
        "event: message_start",
        r#"data: {"type":"message_start","message":{"id":"resp-meta-1","content":[],"model":"claude-test","role":"assistant","type":"message","usage":{}}}"#,
        "",
        "event: content_block_delta",
        r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"ok"}}"#,
        "",
        "event: message_delta",
        r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"input_tokens":4,"output_tokens":1}}"#,
        "",
        "event: message_stop",
        r#"data: {"type":"message_stop"}"#,
        "",
    ]
    .join("\n");

    let metadata_matcher = |req: &Request| {
        req.headers
            .get("x-codex-turn-metadata")
            .and_then(|value| value.to_str().ok())
            == Some(r#"{"turn_id":"turn-123"}"#)
    };
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(metadata_matcher)
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(sse, "text/event-stream"),
        )
        .mount(&server)
        .await;

    let mut request = test_request(server.uri());
    request.turn_metadata_header = Some(r#"{"turn_id":"turn-123"}"#.to_string());

    let mut stream = stream_anthropic(request).await.expect("stream receiver");
    while let Some(event) = stream.recv().await {
        if matches!(
            event.expect("response event"),
            ResponseEvent::Completed { .. }
        ) {
            break;
        }
    }
}

#[tokio::test]
async fn build_request_ignores_available_tools_for_now() {
    let mut request = test_request("https://api.anthropic.com".to_string());
    request.tools.push(codex_tools::ToolSpec::LocalShell {});
    let built = build_request(&request).expect("tools should be ignored for text path");
    assert_eq!(built.messages.len(), 1);
}

#[tokio::test]
async fn stream_anthropic_reports_overloaded_error_events() {
    let server = MockServer::start().await;
    let sse = [
        "event: error",
        r#"data: {"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#,
        "",
    ]
    .join("\n");

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(sse, "text/event-stream"),
        )
        .mount(&server)
        .await;

    let request = test_request(server.uri());
    let mut stream = stream_anthropic(request).await.expect("stream receiver");
    let event = stream.recv().await.expect("expected one error event");
    assert_eq!(
        event.expect_err("expected overloaded error").to_string(),
        "Selected model is at capacity. Please try a different model."
    );
}

#[tokio::test]
async fn stream_anthropic_errors_on_truncated_responses() {
    let server = MockServer::start().await;
    let sse = [
        "event: message_start",
        r#"data: {"type":"message_start","message":{"id":"resp-max-1","content":[],"model":"claude-test","role":"assistant","type":"message","usage":{}}}"#,
        "",
        "event: content_block_delta",
        r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"partial"}}"#,
        "",
        "event: message_delta",
        r#"data: {"type":"message_delta","delta":{"stop_reason":"max_tokens","stop_sequence":null},"usage":{"input_tokens":4,"output_tokens":1}}"#,
        "",
        "event: message_stop",
        r#"data: {"type":"message_stop"}"#,
        "",
    ]
    .join("\n");

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(sse, "text/event-stream"),
        )
        .mount(&server)
        .await;

    let request = test_request(server.uri());
    let mut stream = stream_anthropic(request).await.expect("stream receiver");
    let mut saw_partial_text = false;
    let mut saw_error = false;

    while let Some(event) = stream.recv().await {
        match event {
            Ok(ResponseEvent::OutputTextDelta(delta)) => {
                saw_partial_text = true;
                assert_eq!(delta, "partial");
            }
            Err(err) => {
                saw_error = true;
                assert!(err.to_string().contains("max_tokens"));
                break;
            }
            _ => {}
        }
    }

    assert!(saw_partial_text);
    assert!(saw_error);
}

#[tokio::test]
async fn stream_anthropic_errors_on_incomplete_tool_use_blocks() {
    let server = MockServer::start().await;
    let sse = [
        "event: message_start",
        r#"data: {"type":"message_start","message":{"id":"resp-tool-max-1","content":[],"model":"claude-test","role":"assistant","type":"message","usage":{}}}"#,
        "",
        "event: content_block_start",
        r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"call_time_1","name":"time","input":{}}}"#,
        "",
        "event: content_block_delta",
        r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"utc_offset\":\"+00"}}"#,
        "",
        "event: message_delta",
        r#"data: {"type":"message_delta","delta":{"stop_reason":"max_tokens","stop_sequence":null},"usage":{"input_tokens":4,"output_tokens":1}}"#,
        "",
        "event: message_stop",
        r#"data: {"type":"message_stop"}"#,
        "",
    ]
    .join("\n");

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(sse, "text/event-stream"),
        )
        .mount(&server)
        .await;

    let mut request = test_request(server.uri());
    request.tools.push(codex_tools::ToolSpec::Function(
        codex_tools::ResponsesApiTool {
            name: "time".to_string(),
            description: "Returns current time".to_string(),
            strict: false,
            defer_loading: None,
            parameters: codex_tools::JsonSchema::object(
                std::collections::BTreeMap::from([(
                    "utc_offset".to_string(),
                    codex_tools::JsonSchema::string(None),
                )]),
                Some(vec!["utc_offset".to_string()]),
                Some(codex_tools::AdditionalProperties::Boolean(false)),
            ),
            output_schema: None,
        },
    ));

    let mut stream = stream_anthropic(request).await.expect("stream receiver");
    while let Some(event) = stream.recv().await {
        match event {
            Err(err) => {
                assert!(err.to_string().contains("content_block_stop"));
                return;
            }
            Ok(ResponseEvent::Completed { .. }) => {
                panic!("incomplete tool_use must not complete successfully");
            }
            _ => {}
        }
    }

    panic!("expected stream error for incomplete tool_use");
}

#[tokio::test]
async fn stream_anthropic_includes_cache_write_tokens_in_usage() {
    let server = MockServer::start().await;
    let sse = [
        "event: message_start",
        r#"data: {"type":"message_start","message":{"id":"resp-usage-1","content":[],"model":"claude-test","role":"assistant","type":"message","usage":{}}}"#,
        "",
        "event: content_block_delta",
        r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"ok"}}"#,
        "",
        "event: message_delta",
        r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"input_tokens":4,"cache_creation_input_tokens":3,"cache_read_input_tokens":2,"output_tokens":1}}"#,
        "",
        "event: message_stop",
        r#"data: {"type":"message_stop"}"#,
        "",
    ]
    .join("\n");

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(sse, "text/event-stream"),
        )
        .mount(&server)
        .await;

    let request = test_request(server.uri());
    let mut stream = stream_anthropic(request).await.expect("stream receiver");

    while let Some(event) = stream.recv().await {
        if let Ok(ResponseEvent::Completed { token_usage, .. }) = event {
            let token_usage = token_usage.expect("token usage should be present");
            assert_eq!(token_usage.input_tokens, 4);
            assert_eq!(token_usage.cached_input_tokens, 5);
            assert_eq!(token_usage.total_tokens, 10);
            return;
        }
    }

    panic!("expected completed event with token usage");
}
