use super::*;
use crate::ChatToolCallKind;
use assert_matches::assert_matches;
use bytes::Bytes;
use codex_client::TransportError;
use codex_protocol::ResponseItemId;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ReasoningItemContent;
use codex_protocol::models::ResponseItem;
use futures::StreamExt;
use futures::TryStreamExt;
use futures::stream;
use pretty_assertions::assert_eq;
use serde_json::json;
use tokio::time::sleep;
use tokio_util::io::ReaderStream;

fn chat_sse(events: &[Value]) -> String {
    let mut body = events
        .iter()
        .map(|event| format!("data: {event}\n\n"))
        .collect::<String>();
    body.push_str("data: [DONE]\n\n");
    body
}

async fn collect_events(
    body: String,
    tool_call_info: HashMap<String, ChatToolCallInfo>,
) -> Vec<Result<ResponseEvent, ApiError>> {
    let stream = ReaderStream::new(std::io::Cursor::new(body))
        .map_err(|error| TransportError::Network(error.to_string()));
    let (tx, mut rx) = mpsc::channel(32);
    tokio::spawn(process_sse(
        Box::pin(stream),
        tx,
        Duration::from_secs(1),
        /*telemetry*/ None,
        tool_call_info,
    ));
    let mut events = Vec::new();
    while let Some(event) = rx.recv().await {
        events.push(event);
    }
    events
}

#[tokio::test]
async fn accumulates_text_reasoning_usage_and_stop_reason() {
    let events = collect_events(
        chat_sse(&[
            json!({
                "id": "chatcmpl_1",
                "choices": [{
                    "index": 0,
                    "delta": {"role": "assistant", "reasoning_content": "think "}
                }]
            }),
            json!({
                "choices": [{
                    "index": 0,
                    "delta": {"reasoning_content": "step"}
                }]
            }),
            json!({
                "choices": [{
                    "index": 0,
                    "delta": {"content": "hello"}
                }]
            }),
            json!({
                "choices": [{
                    "index": 0,
                    "delta": {"content": " world"},
                    "finish_reason": "stop"
                }]
            }),
            json!({
                "choices": [],
                "usage": {
                    "prompt_tokens": 10,
                    "completion_tokens": 4,
                    "total_tokens": 14,
                    "prompt_tokens_details": {"cached_tokens": 3},
                    "completion_tokens_details": {"reasoning_tokens": 2}
                }
            }),
        ]),
        HashMap::new(),
    )
    .await;

    assert_matches!(events.first(), Some(Ok(ResponseEvent::Created)));
    assert_eq!(
        events
            .iter()
            .filter_map(|event| match event {
                Ok(ResponseEvent::ReasoningContentDelta { delta, .. }) => Some(delta.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>(),
        vec!["think ", "step"]
    );
    assert_eq!(
        events
            .iter()
            .filter_map(|event| match event {
                Ok(ResponseEvent::OutputTextDelta(delta)) => Some(delta.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>(),
        vec!["hello", " world"]
    );
    assert_eq!(
        events
            .iter()
            .filter_map(|event| match event {
                Ok(ResponseEvent::OutputItemDone(ResponseItem::Reasoning { content, .. })) => {
                    content.clone()
                }
                _ => None,
            })
            .collect::<Vec<_>>(),
        vec![vec![ReasoningItemContent::ReasoningText {
            text: "think step".to_string(),
        }]]
    );
    assert_eq!(
        events
            .iter()
            .filter_map(|event| match event {
                Ok(ResponseEvent::OutputItemDone(ResponseItem::Message { content, .. })) => {
                    Some(content.clone())
                }
                _ => None,
            })
            .collect::<Vec<_>>(),
        vec![vec![ContentItem::OutputText {
            text: "hello world".to_string(),
        }]]
    );
    assert_matches!(
        events.last(),
        Some(Ok(ResponseEvent::Completed {
            response_id,
            token_usage: Some(TokenUsage {
                input_tokens: 10,
                cached_input_tokens: 3,
                output_tokens: 4,
                reasoning_output_tokens: 2,
                total_tokens: 14,
                ..
            }),
            end_turn: Some(true),
            provider_stop_reason: Some(reason),
        })) if response_id == "chatcmpl_1" && reason == "stop"
    );
    let added_reasoning_id = events.iter().find_map(|event| match event {
        Ok(ResponseEvent::OutputItemAdded(ResponseItem::Reasoning { id, .. })) => id.clone(),
        _ => None,
    });
    let completed_reasoning_id = events.iter().find_map(|event| match event {
        Ok(ResponseEvent::OutputItemDone(ResponseItem::Reasoning { id, .. })) => id.clone(),
        _ => None,
    });
    assert_eq!(added_reasoning_id, completed_reasoning_id);
    let reasoning_done = events
        .iter()
        .position(|event| {
            matches!(
                event,
                Ok(ResponseEvent::OutputItemDone(
                    ResponseItem::Reasoning { .. }
                ))
            )
        })
        .expect("reasoning done event");
    let assistant_added = events
        .iter()
        .position(|event| {
            matches!(
                event,
                Ok(ResponseEvent::OutputItemAdded(ResponseItem::Message { .. }))
            )
        })
        .expect("assistant added event");
    assert!(reasoning_done < assistant_added);
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, Ok(ResponseEvent::Completed { .. })))
            .count(),
        1
    );
}

#[tokio::test]
async fn derives_total_usage_when_compatible_provider_omits_it() {
    let events = collect_events(
        chat_sse(&[
            json!({
                "id": "chatcmpl_usage_without_total",
                "choices": [{
                    "index": 0,
                    "delta": {"content": "hello"},
                    "finish_reason": "stop"
                }]
            }),
            json!({
                "choices": [],
                "usage": {
                    "prompt_tokens": 10,
                    "completion_tokens": 4
                }
            }),
        ]),
        HashMap::new(),
    )
    .await;

    assert_matches!(
        events.last(),
        Some(Ok(ResponseEvent::Completed {
            token_usage: Some(TokenUsage {
                input_tokens: 10,
                output_tokens: 4,
                total_tokens: 14,
                ..
            }),
            ..
        }))
    );
}

#[tokio::test]
async fn reasoning_aliases_and_null_fallback_emit_the_same_delta() {
    for field in ["reasoning_content", "reasoning", "thinking"] {
        let mut delta = serde_json::Map::new();
        delta.insert(field.to_string(), json!("think"));
        let events = collect_events(
            chat_sse(&[json!({
                "id": "chatcmpl_reasoning",
                "choices": [{
                    "index": 0,
                    "delta": Value::Object(delta),
                    "finish_reason": "stop"
                }]
            })]),
            HashMap::new(),
        )
        .await;
        assert!(events.iter().any(|event| {
            matches!(event, Ok(ResponseEvent::ReasoningContentDelta { delta, .. }) if delta == "think")
        }));
    }

    let events = collect_events(
        chat_sse(&[json!({
            "id": "chatcmpl_reasoning_fallback",
            "choices": [{
                "index": 0,
                "delta": {"reasoning_content": null, "reasoning": "fallback"},
                "finish_reason": "stop"
            }]
        })]),
        HashMap::new(),
    )
    .await;
    assert!(events.iter().any(|event| {
        matches!(event, Ok(ResponseEvent::ReasoningContentDelta { delta, .. }) if delta == "fallback")
    }));
}

#[tokio::test]
async fn keeps_multiple_tool_call_arguments_separate_and_restores_namespaces() {
    let tool_call_info = HashMap::from([
        (
            "app__first".to_string(),
            ChatToolCallInfo {
                name: "first".to_string(),
                namespace: Some("app".to_string()),
                kind: ChatToolCallKind::Function,
            },
        ),
        (
            "second".to_string(),
            ChatToolCallInfo {
                name: "second".to_string(),
                namespace: None,
                kind: ChatToolCallKind::Function,
            },
        ),
    ]);
    let events = collect_events(
        chat_sse(&[
            json!({
                "id": "chatcmpl_tools",
                "choices": [{"index": 0, "delta": {"tool_calls": [
                    {"index": 4, "id": "call_a", "function": {"name": "app__first", "arguments": "{\"a\":"}},
                    {"index": 9, "id": "call_b", "function": {"name": "second", "arguments": "{\"b\":"}}
                ]}}]
            }),
            json!({
                "choices": [{"index": 0, "delta": {"tool_calls": [
                    {"index": 9, "function": {"arguments": "2}"}},
                    {"index": 4, "function": {"arguments": "1}"}}
                ]}, "finish_reason": "tool_calls"}]
            }),
        ]),
        tool_call_info,
    )
    .await;

    let items = events
        .iter()
        .filter_map(|event| match event {
            Ok(ResponseEvent::OutputItemDone(item @ ResponseItem::FunctionCall { .. })) => {
                Some(item.clone())
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        events
            .iter()
            .filter_map(|event| match event {
                Ok(ResponseEvent::ToolCallInputDelta {
                    item_id,
                    call_id,
                    delta,
                }) => Some((item_id.clone(), call_id.clone(), delta.clone())),
                _ => None,
            })
            .collect::<Vec<_>>(),
        vec![
            (
                "call_a".to_string(),
                Some("call_a".to_string()),
                "{\"a\":".to_string(),
            ),
            (
                "call_b".to_string(),
                Some("call_b".to_string()),
                "{\"b\":".to_string(),
            ),
            (
                "call_b".to_string(),
                Some("call_b".to_string()),
                "2}".to_string(),
            ),
            (
                "call_a".to_string(),
                Some("call_a".to_string()),
                "1}".to_string(),
            ),
        ]
    );
    assert_eq!(
        items,
        vec![
            ResponseItem::FunctionCall {
                id: Some(ResponseItemId::with_suffix("fc", "call_a")),
                name: "first".to_string(),
                namespace: Some("app".to_string()),
                arguments: "{\"a\":1}".to_string(),
                call_id: "call_a".to_string(),
                internal_chat_message_metadata_passthrough: None,
            },
            ResponseItem::FunctionCall {
                id: Some(ResponseItemId::with_suffix("fc", "call_b")),
                name: "second".to_string(),
                namespace: None,
                arguments: "{\"b\":2}".to_string(),
                call_id: "call_b".to_string(),
                internal_chat_message_metadata_passthrough: None,
            },
        ]
    );
    assert_matches!(
        events.last(),
        Some(Ok(ResponseEvent::Completed {
            end_turn: Some(false),
            provider_stop_reason: Some(reason),
            ..
        })) if reason == "tool_calls"
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, Ok(ResponseEvent::Completed { .. })))
            .count(),
        1
    );
}

#[tokio::test]
async fn separates_indexless_tool_calls_when_follow_up_omits_ids() {
    let events = collect_events(
        chat_sse(&[
            json!({
                "id": "chatcmpl_indexless_tools",
                "choices": [{"index": 0, "delta": {"tool_calls": [
                    {"id": "call_a", "function": {"name": "first", "arguments": "{\"a\":"}},
                    {"id": "call_b", "function": {"name": "second", "arguments": "{\"b\":"}}
                ]}}]
            }),
            json!({
                "choices": [{"index": 0, "delta": {"tool_calls": [
                    {"function": {"arguments": "1"}},
                    {"function": {"arguments": "2"}}
                ]}}]
            }),
            json!({
                "choices": [{"index": 0, "delta": {"tool_calls": [
                    {"id": "call_b", "function": {"arguments": "}"}},
                    {"id": "call_a", "function": {"arguments": "}"}}
                ]}, "finish_reason": "tool_calls"}]
            }),
        ]),
        HashMap::new(),
    )
    .await;

    let items = events
        .iter()
        .filter_map(|event| match event {
            Ok(ResponseEvent::OutputItemDone(item @ ResponseItem::FunctionCall { .. })) => {
                Some(item.clone())
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        items,
        vec![
            ResponseItem::FunctionCall {
                id: Some(ResponseItemId::with_suffix("fc", "call_a")),
                name: "first".to_string(),
                namespace: None,
                arguments: "{\"a\":1}".to_string(),
                call_id: "call_a".to_string(),
                internal_chat_message_metadata_passthrough: None,
            },
            ResponseItem::FunctionCall {
                id: Some(ResponseItemId::with_suffix("fc", "call_b")),
                name: "second".to_string(),
                namespace: None,
                arguments: "{\"b\":2}".to_string(),
                call_id: "call_b".to_string(),
                internal_chat_message_metadata_passthrough: None,
            },
        ]
    );
}

#[tokio::test]
async fn separates_indexless_tool_call_from_an_occupied_dense_index() {
    let events = collect_events(
        chat_sse(&[json!({
            "id": "chatcmpl_mixed_tool_indexes",
            "choices": [{"index": 0, "delta": {"tool_calls": [
                {
                    "index": 1,
                    "id": "call_explicit",
                    "function": {"name": "first", "arguments": "{}"}
                },
                {
                    "id": "call_indexless",
                    "function": {"name": "second", "arguments": "{}"}
                }
            ]}, "finish_reason": "tool_calls"}]
        })]),
        HashMap::new(),
    )
    .await;

    let items = events
        .iter()
        .filter_map(|event| match event {
            Ok(ResponseEvent::OutputItemDone(item @ ResponseItem::FunctionCall { .. })) => {
                Some(item.clone())
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        items,
        vec![
            ResponseItem::FunctionCall {
                id: Some(ResponseItemId::with_suffix("fc", "call_explicit")),
                name: "first".to_string(),
                namespace: None,
                arguments: "{}".to_string(),
                call_id: "call_explicit".to_string(),
                internal_chat_message_metadata_passthrough: None,
            },
            ResponseItem::FunctionCall {
                id: Some(ResponseItemId::with_suffix("fc", "call_indexless")),
                name: "second".to_string(),
                namespace: None,
                arguments: "{}".to_string(),
                call_id: "call_indexless".to_string(),
                internal_chat_message_metadata_passthrough: None,
            },
        ]
    );
}

#[tokio::test]
async fn restores_custom_tool_input_from_function_wrapper() {
    let events = collect_events(
        chat_sse(&[json!({
            "id": "chatcmpl_custom",
            "choices": [{
                "index": 0,
                "delta": {"tool_calls": [{
                    "index": 0,
                    "id": "call_patch",
                    "function": {
                        "name": "apply_patch",
                        "arguments": "{\"input\":\"*** Begin Patch\"}"
                    }
                }]},
                "finish_reason": "tool_calls"
            }]
        })]),
        HashMap::from([(
            "apply_patch".to_string(),
            ChatToolCallInfo {
                name: "apply_patch".to_string(),
                namespace: None,
                kind: ChatToolCallKind::Custom,
            },
        )]),
    )
    .await;

    assert!(events.iter().any(|event| {
        matches!(
            event,
            Ok(ResponseEvent::OutputItemDone(ResponseItem::CustomToolCall {
                call_id,
                name,
                input,
                ..
            })) if call_id == "call_patch" && name == "apply_patch" && input == "*** Begin Patch"
        )
    }));
}

#[tokio::test]
async fn restores_tool_search_arguments_from_function_wrapper() {
    let events = collect_events(
        chat_sse(&[json!({
            "id": "chatcmpl_search",
            "choices": [{
                "index": 0,
                "delta": {"tool_calls": [{
                    "index": 0,
                    "id": "search_1",
                    "function": {
                        "name": "tool_search_wire",
                        "arguments": "{\"query\":\"calendar\"}"
                    }
                }]},
                "finish_reason": "tool_calls"
            }]
        })]),
        HashMap::from([(
            "tool_search_wire".to_string(),
            ChatToolCallInfo {
                name: "tool_search".to_string(),
                namespace: None,
                kind: ChatToolCallKind::ToolSearch,
            },
        )]),
    )
    .await;

    assert!(events.iter().any(|event| {
        matches!(
            event,
            Ok(ResponseEvent::OutputItemDone(ResponseItem::ToolSearchCall {
                call_id: Some(call_id),
                arguments,
                ..
            })) if call_id == "search_1" && arguments == &json!({"query": "calendar"})
        )
    }));
}

#[tokio::test]
async fn provider_error_is_actionable() {
    let events = collect_events(
        format!(
            "data: {}\n\n",
            json!({"error": {"type": "invalid_request_error", "message": "bad prompt"}})
        ),
        HashMap::new(),
    )
    .await;

    assert_matches!(
        events.as_slice(),
        [Err(ApiError::StreamFailure {
            kind: ProviderStreamErrorKind::ProviderError,
            message,
        })] if message.contains("invalid_request_error") && message.contains("bad prompt")
    );
}

#[tokio::test]
async fn empty_stream_is_classified_before_message_start() {
    let events = collect_events(String::new(), HashMap::new()).await;

    assert_matches!(
        events.as_slice(),
        [Err(ApiError::StreamFailure {
            kind: ProviderStreamErrorKind::ClosedBeforeMessageStart,
            message,
        })] if message.contains("before a terminal finish reason")
    );
}

#[tokio::test]
async fn malformed_chunk_is_classified_as_parse_error() {
    let events = collect_events("data: {\n\n".to_string(), HashMap::new()).await;

    assert_matches!(
        events.as_slice(),
        [Err(ApiError::StreamFailure {
            kind: ProviderStreamErrorKind::ParseError,
            message,
        })] if message.contains("failed to parse Chat SSE JSON")
    );
}

#[tokio::test(start_paused = true)]
async fn non_meaningful_frames_do_not_reset_idle_deadline() {
    let stream = stream::unfold((), |_| async {
        sleep(Duration::from_millis(5)).await;
        Some((
            Ok::<_, TransportError>(Bytes::from_static(
                b"data: {\"id\":\"same\",\"choices\":[{\"delta\":{\"tool_calls\":[{}]}}]}\n\n",
            )),
            (),
        ))
    });
    let (tx, mut rx) = mpsc::channel(8);
    tokio::spawn(process_sse(
        Box::pin(stream),
        tx,
        Duration::from_millis(20),
        /*telemetry*/ None,
        HashMap::new(),
    ));

    let created = timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("Chat idle test should finish")
        .expect("Chat idle test should emit Created");
    assert_matches!(created, Ok(ResponseEvent::Created));
    let event = timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("Chat idle test should finish")
        .expect("Chat idle test should emit an error after Created");
    assert_matches!(
        event,
        Err(ApiError::StreamFailure {
            kind: ProviderStreamErrorKind::IdleTimeout,
            ..
        })
    );
}

#[tokio::test(start_paused = true)]
async fn meaningful_frames_extend_idle_deadline() {
    let chunks = [
        Bytes::from_static(
            b"data: {\"id\":\"chat_progress\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"one\"}}]}\n\n",
        ),
        Bytes::from_static(
            b"data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\" two\"}}]}\n\n",
        ),
        Bytes::from_static(
            b"data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\" three\"},\"finish_reason\":\"stop\"}]}\n\n",
        ),
    ];
    let stream = stream::iter(chunks).then(|chunk| async move {
        sleep(Duration::from_millis(15)).await;
        Ok::<_, TransportError>(chunk)
    });
    let (tx, mut rx) = mpsc::channel(16);
    tokio::spawn(process_sse(
        Box::pin(stream),
        tx,
        Duration::from_millis(20),
        /*telemetry*/ None,
        HashMap::new(),
    ));

    let mut events = Vec::new();
    while let Some(event) = rx.recv().await {
        events.push(event);
    }

    assert_matches!(
        events.last(),
        Some(Ok(ResponseEvent::Completed { response_id, .. }))
            if response_id == "chat_progress"
    );
}

#[tokio::test]
async fn accepts_model_context_items_between_legacy_and_current_limits() {
    // Above the legacy 10KB byte cap, under the current per-item token cap, and within
    // the response cumulative budget that leaves JSON headroom for the next request.
    let fragment = "x".repeat(10_500);
    let events = collect_events(
        chat_sse(&[
            json!({
                "id": "chat_bounded_context",
                "choices": [{"index": 0, "delta": {"reasoning_content": fragment}}]
            }),
            json!({
                "choices": [{"index": 0, "delta": {"content": fragment}}]
            }),
            json!({
                "choices": [{"index": 0, "delta": {"tool_calls": [{
                    "index": 0,
                    "id": "call_bounded",
                    "function": {"name": "tool", "arguments": fragment}
                }]}, "finish_reason": "tool_calls"}]
            }),
        ]),
        HashMap::new(),
    )
    .await;

    assert!(events.iter().all(Result::is_ok));
    assert_matches!(
        events.last(),
        Some(Ok(ResponseEvent::Completed { response_id, .. }))
            if response_id == "chat_bounded_context"
    );
}

#[tokio::test]
async fn rejects_unbounded_model_context_items() {
    let fragment = "x".repeat(MAX_CHAT_CONTEXT_ITEM_BYTES / 2 + 1);
    let cases = [
        (
            "assistant text",
            vec![
                json!({
                    "id": "chat_oversized_text",
                    "choices": [{"index": 0, "delta": {"content": fragment}}]
                }),
                json!({
                    "choices": [{"index": 0, "delta": {"content": fragment}, "finish_reason": "stop"}]
                }),
            ],
        ),
        (
            "reasoning content",
            vec![
                json!({
                    "id": "chat_oversized_reasoning",
                    "choices": [{"index": 0, "delta": {"reasoning_content": fragment}}]
                }),
                json!({
                    "choices": [{"index": 0, "delta": {"reasoning_content": fragment}, "finish_reason": "stop"}]
                }),
            ],
        ),
        (
            "tool-call arguments",
            vec![
                json!({
                    "id": "chat_oversized_tool",
                    "choices": [{"index": 0, "delta": {"tool_calls": [{
                        "index": 0,
                        "id": "call_oversized",
                        "function": {"name": "tool", "arguments": fragment}
                    }]}}]
                }),
                json!({
                    "choices": [{"index": 0, "delta": {"tool_calls": [{
                        "index": 0,
                        "function": {"arguments": fragment}
                    }]}, "finish_reason": "tool_calls"}]
                }),
            ],
        ),
    ];

    for (item_kind, chunks) in cases {
        let events = collect_events(chat_sse(&chunks), HashMap::new()).await;
        assert!(events.iter().any(|event| {
            matches!(
                event,
                Err(ApiError::StreamFailure {
                    kind: ProviderStreamErrorKind::ParseError,
                    message,
                }) if message.contains(item_kind) && message.contains("context limit")
            )
        }));
    }
}

#[tokio::test]
async fn rejects_oversized_provider_identifiers() {
    let oversized_id = "x".repeat(MAX_CHAT_WIRE_IDENTIFIER_BYTES + 1);
    let events = collect_events(
        chat_sse(&[json!({
            "id": "chat_identifier_limit",
            "choices": [{"index": 0, "delta": {"tool_calls": [{
                "index": 0,
                "id": oversized_id,
                "function": {"name": "tool", "arguments": "{}"}
            }]}, "finish_reason": "tool_calls"}]
        })]),
        HashMap::new(),
    )
    .await;

    assert!(events.iter().any(|event| {
        matches!(
            event,
            Err(ApiError::StreamFailure {
                kind: ProviderStreamErrorKind::ParseError,
                message,
            }) if message.contains("tool-call ID") && message.contains("identifier limit")
        )
    }));
}
