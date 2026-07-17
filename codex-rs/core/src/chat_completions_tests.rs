use super::*;
use codex_protocol::models::AgentMessageInputContent;
use codex_protocol::models::BaseInstructions;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::ImageDetail;
use codex_tools::AdditionalProperties;
use codex_tools::JsonSchema;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolSpec;
use pretty_assertions::assert_eq;
use std::collections::BTreeMap;

fn model_info() -> ModelInfo {
    serde_json::from_value(json!({
        "slug": "chat-model",
        "display_name": "Chat Model",
        "description": "desc",
        "default_reasoning_level": null,
        "supported_reasoning_levels": [],
        "shell_type": "shell_command",
        "visibility": "list",
        "supported_in_api": true,
        "priority": 1,
        "upgrade": null,
        "base_instructions": "base instructions",
        "model_messages": null,
        "support_verbosity": false,
        "default_verbosity": null,
        "apply_patch_tool_type": null,
        "truncation_policy": {"mode": "bytes", "limit": 10000},
        "supports_parallel_tool_calls": true,
        "supports_image_detail_original": true,
        "context_window": 128000,
        "auto_compact_token_limit": null,
        "experimental_supported_tools": []
    }))
    .expect("deserialize model info")
}

fn message(role: &str, content: Vec<ContentItem>) -> ResponseItem {
    ResponseItem::Message {
        id: None,
        role: role.to_string(),
        content,
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    }
}

#[test]
fn serializes_complete_text_request() {
    let prompt = Prompt {
        input: vec![message(
            "user",
            vec![ContentItem::InputText {
                text: "hello".to_string(),
            }],
        )],
        base_instructions: BaseInstructions {
            text: "be concise".to_string(),
        },
        output_schema: Some(json!({
            "type": "object",
            "properties": {"answer": {"type": "string"}},
            "required": ["answer"],
            "additionalProperties": false
        })),
        ..Default::default()
    };

    let request = build_chat_completions_request(
        &prompt,
        &model_info(),
        Some(ReasoningEffort::High),
        Some("priority".to_string()),
    )
    .expect("build Chat request");

    assert_eq!(
        serde_json::to_value(request).expect("serialize Chat request"),
        json!({
            "model": "chat-model",
            "messages": [
                {"role": "system", "content": "be concise"},
                {"role": "user", "content": "hello"}
            ],
            "stream": true,
            "stream_options": {"include_usage": true},
            "reasoning_effort": "high",
            "service_tier": "priority",
            "response_format": {
                "type": "json_schema",
                "json_schema": {
                    "name": "codex_output_schema",
                    "strict": true,
                    "schema": {
                        "type": "object",
                        "properties": {"answer": {"type": "string"}},
                        "required": ["answer"],
                        "additionalProperties": false
                    }
                }
            }
        })
    );
}

#[test]
fn merges_assistant_text_and_tool_calls_into_one_message() {
    let lookup_name = chat_tool_name(
        /*namespace*/ None,
        "lookup",
        ChatToolCallKind::Function,
    );
    let prompt = Prompt {
        input: vec![
            message(
                "user",
                vec![ContentItem::InputText {
                    text: "find it".to_string(),
                }],
            ),
            message(
                "assistant",
                vec![ContentItem::OutputText {
                    text: "looking it up".to_string(),
                }],
            ),
            ResponseItem::FunctionCall {
                id: None,
                name: "lookup".to_string(),
                namespace: None,
                arguments: "{\"key\":\"a\"}".to_string(),
                call_id: "call_1".to_string(),
                internal_chat_message_metadata_passthrough: None,
            },
            ResponseItem::FunctionCallOutput {
                id: None,
                call_id: "call_1".to_string(),
                output: FunctionCallOutputPayload::from_text("value".to_string()),
                internal_chat_message_metadata_passthrough: None,
            },
        ],
        base_instructions: BaseInstructions {
            text: String::new(),
        },
        ..Default::default()
    };

    let request = build_chat_completions_request(&prompt, &model_info(), None, None)
        .expect("build Chat request");

    assert_eq!(
        serde_json::to_value(request.messages).expect("serialize Chat messages"),
        json!([
            {"role": "user", "content": "find it"},
            {
                "role": "assistant",
                "content": "looking it up",
                "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": {"name": lookup_name, "arguments": "{\"key\":\"a\"}"}
                }]
            },
            {"role": "tool", "content": "value", "tool_call_id": "call_1"}
        ])
    );
}

#[test]
fn serializes_complete_tool_history_request() {
    let lookup_name = chat_tool_name(
        /*namespace*/ None,
        "lookup",
        ChatToolCallKind::Function,
    );
    let tool = ToolSpec::Function(ResponsesApiTool {
        name: "lookup".to_string(),
        description: "Look up a value".to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(
            BTreeMap::from([("key".to_string(), JsonSchema::string(/*description*/ None))]),
            Some(vec!["key".to_string()]),
            Some(AdditionalProperties::Boolean(false)),
        ),
        output_schema: None,
    });
    let prompt = Prompt {
        input: vec![
            message(
                "user",
                vec![ContentItem::InputText {
                    text: "find it".to_string(),
                }],
            ),
            ResponseItem::FunctionCall {
                id: None,
                name: "lookup".to_string(),
                namespace: None,
                arguments: "{\"key\":\"a\"}".to_string(),
                call_id: "call_1".to_string(),
                internal_chat_message_metadata_passthrough: None,
            },
            ResponseItem::FunctionCallOutput {
                id: None,
                call_id: "call_1".to_string(),
                output: FunctionCallOutputPayload::from_text("value".to_string()),
                internal_chat_message_metadata_passthrough: None,
            },
        ],
        tools: vec![tool],
        parallel_tool_calls: true,
        base_instructions: BaseInstructions {
            text: String::new(),
        },
        output_schema: None,
        output_schema_strict: true,
    };

    let request = build_chat_completions_request(&prompt, &model_info(), None, None)
        .expect("build Chat request");

    assert_eq!(
        serde_json::to_value(request).expect("serialize Chat request"),
        json!({
            "model": "chat-model",
            "messages": [
                {"role": "user", "content": "find it"},
                {
                    "role": "assistant",
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {"name": lookup_name, "arguments": "{\"key\":\"a\"}"}
                    }]
                },
                {"role": "tool", "content": "value", "tool_call_id": "call_1"}
            ],
            "stream": true,
            "stream_options": {"include_usage": true},
            "tools": [{
                "type": "function",
                "function": {
                    "name": lookup_name,
                    "description": "Look up a value",
                    "parameters": {
                        "type": "object",
                        "properties": {"key": {"type": "string"}},
                        "required": ["key"],
                        "additionalProperties": false
                    }
                }
            }],
            "tool_choice": "auto",
            "parallel_tool_calls": true
        })
    );
}

#[test]
fn serializes_supported_and_unsupported_images_as_explicit_parts() {
    let prompt = Prompt {
        input: vec![message(
            "user",
            vec![
                ContentItem::InputText {
                    text: "compare".to_string(),
                },
                ContentItem::InputImage {
                    image_url: "https://example.com/image.png".to_string(),
                    detail: Some(ImageDetail::High),
                },
                ContentItem::InputImage {
                    image_url: "/tmp/local.png".to_string(),
                    detail: None,
                },
            ],
        )],
        base_instructions: BaseInstructions {
            text: String::new(),
        },
        ..Default::default()
    };

    let request = build_chat_completions_request(&prompt, &model_info(), None, None)
        .expect("build Chat request");

    assert_eq!(
        serde_json::to_value(request).expect("serialize Chat request"),
        json!({
            "model": "chat-model",
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "text", "text": "compare"},
                    {
                        "type": "image_url",
                        "image_url": {"url": "https://example.com/image.png", "detail": "high"}
                    },
                    {"type": "text", "text": UNSUPPORTED_IMAGE_PLACEHOLDER}
                ]
            }],
            "stream": true,
            "stream_options": {"include_usage": true}
        })
    );
}

#[test]
fn rejects_internal_agent_messages() {
    let prompt = Prompt {
        input: vec![ResponseItem::AgentMessage {
            id: None,
            author: "child".to_string(),
            recipient: "parent".to_string(),
            content: vec![AgentMessageInputContent::InputText {
                text: "<MESSAGE>child result</MESSAGE>".to_string(),
            }],
            internal_chat_message_metadata_passthrough: None,
        }],
        base_instructions: BaseInstructions {
            text: String::new(),
        },
        ..Default::default()
    };

    let error = build_chat_completions_request(&prompt, &model_info(), None, None)
        .expect_err("internal agent messages should be rejected");

    assert!(error.to_string().contains("internal agent messages"));
}

#[test]
fn rejects_oversized_request_items_from_existing_history() {
    let oversized_message = Prompt {
        input: vec![message(
            "user",
            vec![ContentItem::InputText {
                text: "x".repeat(50_000),
            }],
        )],
        base_instructions: BaseInstructions {
            text: String::new(),
        },
        ..Default::default()
    };
    let error = build_chat_completions_request(&oversized_message, &model_info(), None, None)
        .expect_err("oversized message should be rejected");
    assert!(error.to_string().contains("message 0"));

    let merged_tool_calls = Prompt {
        input: vec![
            ResponseItem::FunctionCall {
                id: None,
                name: "first".to_string(),
                namespace: None,
                arguments: "x".repeat(25_000),
                call_id: "call_1".to_string(),
                internal_chat_message_metadata_passthrough: None,
            },
            ResponseItem::FunctionCall {
                id: None,
                name: "second".to_string(),
                namespace: None,
                arguments: "y".repeat(25_000),
                call_id: "call_2".to_string(),
                internal_chat_message_metadata_passthrough: None,
            },
        ],
        base_instructions: BaseInstructions {
            text: String::new(),
        },
        ..Default::default()
    };
    let error = build_chat_completions_request(&merged_tool_calls, &model_info(), None, None)
        .expect_err("merged oversized tool message should be rejected");
    assert!(error.to_string().contains("message 0"));
}

#[test]
fn reasoning_only_history_is_flushed_before_the_next_user_boundary() {
    let prompt = Prompt {
        input: vec![
            ResponseItem::Reasoning {
                id: None,
                summary: Vec::new(),
                content: Some(vec![ReasoningItemContent::ReasoningText {
                    text: "private thought".to_string(),
                }]),
                encrypted_content: None,
                internal_chat_message_metadata_passthrough: None,
            },
            message(
                "user",
                vec![ContentItem::InputText {
                    text: "next turn".to_string(),
                }],
            ),
        ],
        base_instructions: BaseInstructions {
            text: String::new(),
        },
        ..Default::default()
    };

    let request = build_chat_completions_request(&prompt, &model_info(), None, None)
        .expect("build Chat request");

    assert_eq!(
        serde_json::to_value(request.messages).expect("serialize Chat messages"),
        json!([
            {"role": "assistant", "reasoning_content": "private thought"},
            {"role": "user", "content": "next turn"}
        ])
    );
}

#[test]
fn serializes_and_bounds_tool_search_history() {
    let tool_search_name = chat_tool_name(
        /*namespace*/ None,
        "tool_search",
        ChatToolCallKind::ToolSearch,
    );
    let tool = ToolSpec::ToolSearch {
        execution: "client".to_string(),
        description: "Search available tools".to_string(),
        parameters: JsonSchema::object(
            BTreeMap::new(),
            /*required*/ None,
            /*additional_properties*/ None,
        ),
    };
    let prompt = Prompt {
        input: vec![
            message(
                "user",
                vec![ContentItem::InputText {
                    text: "find a tool".to_string(),
                }],
            ),
            ResponseItem::ToolSearchCall {
                id: None,
                call_id: Some("search_1".to_string()),
                status: Some("completed".to_string()),
                execution: "client".to_string(),
                arguments: json!({"query": "calendar"}),
                internal_chat_message_metadata_passthrough: None,
            },
            ResponseItem::ToolSearchOutput {
                id: None,
                call_id: Some("search_1".to_string()),
                status: "completed".to_string(),
                execution: "client".to_string(),
                tools: (0..1_000)
                    .map(|index| json!({"name": format!("tool_{index}"), "description": "x".repeat(40)}))
                    .collect(),
                internal_chat_message_metadata_passthrough: None,
            },
        ],
        tools: vec![tool],
        base_instructions: BaseInstructions {
            text: String::new(),
        },
        ..Default::default()
    };

    let request = build_chat_completions_request(&prompt, &model_info(), None, None)
        .expect("build Chat request");
    assert_eq!(
        request.messages[1].tool_calls[0].function.name,
        tool_search_name
    );
    let Some(ChatMessageContent::Text(output)) = request.messages[2].content.as_ref() else {
        panic!("expected tool search output text");
    };
    assert!(approx_token_count(output) <= MAX_CHAT_CONTEXT_ITEM_TOKENS);
    assert!(output.contains("tokens truncated"));
}
