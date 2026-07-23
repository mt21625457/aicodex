use super::*;
use codex_protocol::models::AgentMessageInputContent;
use codex_protocol::models::BaseInstructions;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::ImageDetail;
use codex_tools::AdditionalProperties;
use codex_tools::FreeformTool;
use codex_tools::FreeformToolFormat;
use codex_tools::JsonSchema;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolSpec;
use pretty_assertions::assert_eq;
use serde_json::Value;
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

fn first_party_file_tool(name: &str) -> ToolSpec {
    let description = match name {
        "read_file" => "Read a bounded text file through Codex's filesystem layer.",
        "edit_file" => "Replace exact text in a previously read text file.",
        "write_file" => "Create or overwrite a bounded text file through Codex's filesystem layer.",
        other => panic!("unexpected dedicated file tool {other}"),
    };
    ToolSpec::Function(ResponsesApiTool {
        name: name.to_string(),
        description: description.to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(
            BTreeMap::from([("path".to_string(), JsonSchema::string(None))]),
            Some(vec!["path".to_string()]),
            Some(AdditionalProperties::Boolean(false)),
        ),
        output_schema: None,
    })
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
        hidden_tools: Vec::new(),
        chat_file_tool_mode: ChatFileToolMode::Legacy,
        claude_file_tool_mode: codex_features::ClaudeFileToolMode::Auto,
        dedicated_file_tools_enabled: false,
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
fn dedicated_request_preserves_hidden_historical_apply_patch_calls() {
    let prompt = Prompt {
        input: vec![
            message(
                "user",
                vec![ContentItem::InputText {
                    text: "continue the resumed edit".to_string(),
                }],
            ),
            ResponseItem::CustomToolCall {
                id: None,
                status: None,
                call_id: "legacy_apply_1".to_string(),
                name: "apply_patch".to_string(),
                namespace: None,
                input: "*** Begin Patch\n*** End Patch".to_string(),
                internal_chat_message_metadata_passthrough: None,
            },
            ResponseItem::CustomToolCallOutput {
                id: None,
                call_id: "legacy_apply_1".to_string(),
                name: Some("apply_patch".to_string()),
                output: FunctionCallOutputPayload::from_text("Done!".to_string()),
                internal_chat_message_metadata_passthrough: None,
            },
        ],
        tools: vec![
            first_party_file_tool("read_file"),
            first_party_file_tool("edit_file"),
            first_party_file_tool("write_file"),
        ],
        chat_file_tool_mode: ChatFileToolMode::Dedicated,
        dedicated_file_tools_enabled: true,
        parallel_tool_calls: true,
        ..Default::default()
    };

    let request = build_chat_completions_request(&prompt, &model_info(), None, None)
        .expect("build dedicated Chat request");
    let value = serde_json::to_value(request).expect("serialize Chat request");
    let historical_name = chat_tool_name(
        /*namespace*/ None,
        "apply_patch",
        ChatToolCallKind::Custom,
    );

    assert!(
        value["tools"]
            .as_array()
            .is_some_and(|tools| tools.iter().all(|tool| {
                tool.pointer("/function/name").and_then(Value::as_str)
                    != Some(historical_name.as_str())
            }))
    );
    let messages = value["messages"].as_array().expect("messages");
    let assistant_call = messages
        .iter()
        .find(|message| message["role"] == "assistant" && message.get("tool_calls").is_some())
        .expect("historical assistant tool call");
    assert_eq!(
        assistant_call["tool_calls"][0]["function"]["name"],
        historical_name
    );
    assert!(messages.iter().any(|message| {
        message["role"] == "tool" && message["tool_call_id"] == "legacy_apply_1"
    }));
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

#[test]
fn dedicated_guidance_uses_actual_hashed_wire_names() {
    let info = vec![
        ChatToolCallInfo {
            chat_name: "read_file__abc".to_string(),
            name: "read_file".to_string(),
            namespace: None,
            kind: ChatToolCallKind::Function,
        },
        ChatToolCallInfo {
            chat_name: "edit_file__def".to_string(),
            name: "edit_file".to_string(),
            namespace: None,
            kind: ChatToolCallKind::Function,
        },
        ChatToolCallInfo {
            chat_name: "write_file__ghi".to_string(),
            name: "write_file".to_string(),
            namespace: None,
            kind: ChatToolCallKind::Function,
        },
    ];
    let prompt = Prompt {
        tools: ["read_file", "edit_file", "write_file"]
            .map(first_party_file_tool)
            .to_vec(),
        chat_file_tool_mode: ChatFileToolMode::Dedicated,
        dedicated_file_tools_enabled: true,
        ..Default::default()
    };
    let guidance =
        dedicated_chat_guidance(&prompt, &info).expect("all first-party mappings are present");
    assert!(guidance.contains("read_file__abc"));
    assert!(guidance.contains("edit_file__def"));
    assert!(guidance.contains("write_file__ghi"));
}

#[test]
fn dedicated_guidance_fails_closed_on_duplicate_reverse_mapping() {
    let info = vec![
        ChatToolCallInfo {
            chat_name: "read_file__abc".to_string(),
            name: "read_file".to_string(),
            namespace: None,
            kind: ChatToolCallKind::Function,
        },
        ChatToolCallInfo {
            chat_name: "read_file__third_party".to_string(),
            name: "read_file".to_string(),
            namespace: None,
            kind: ChatToolCallKind::Function,
        },
    ];
    let prompt = Prompt {
        tools: ["read_file", "edit_file", "write_file"]
            .map(first_party_file_tool)
            .to_vec(),
        chat_file_tool_mode: ChatFileToolMode::Dedicated,
        dedicated_file_tools_enabled: true,
        ..Default::default()
    };
    assert!(dedicated_chat_guidance(&prompt, &info).is_err());
}

#[test]
fn dedicated_guidance_is_bounded_stable_single_copy_and_legacy_safe() {
    let third_party = |name: &str| {
        ToolSpec::Function(ResponsesApiTool {
            name: name.to_string(),
            description: format!("third-party {name}"),
            strict: false,
            defer_loading: None,
            parameters: JsonSchema::object(
                BTreeMap::new(),
                Some(Vec::new()),
                Some(AdditionalProperties::Boolean(false)),
            ),
            output_schema: None,
        })
    };
    let third_party_tools = ["read_file", "edit_file", "write_file"]
        .map(third_party)
        .to_vec();
    let third_party_dedicated = Prompt {
        input: vec![message(
            "user",
            vec![ContentItem::InputText {
                text: "edit safely".to_string(),
            }],
        )],
        tools: third_party_tools.clone(),
        chat_file_tool_mode: ChatFileToolMode::Dedicated,
        dedicated_file_tools_enabled: true,
        ..Default::default()
    };
    assert!(
        build_chat_completions_request(&third_party_dedicated, &model_info(), None, None).is_err(),
        "same-named third-party tools must not unlock dedicated guidance"
    );

    let tools = ["read_file", "edit_file", "write_file"]
        .map(first_party_file_tool)
        .to_vec();
    let dedicated = Prompt {
        input: vec![message(
            "user",
            vec![ContentItem::InputText {
                text: "edit safely".to_string(),
            }],
        )],
        tools: tools,
        chat_file_tool_mode: ChatFileToolMode::Dedicated,
        dedicated_file_tools_enabled: true,
        ..Default::default()
    };
    let first = build_chat_completions_request(&dedicated, &model_info(), None, None)
        .expect("first dedicated request");
    let second = build_chat_completions_request(&dedicated, &model_info(), None, None)
        .expect("retry dedicated request");
    let first_value = serde_json::to_value(&first).expect("serialize first request");
    let second_value = serde_json::to_value(&second).expect("serialize second request");
    assert_eq!(first_value, second_value);
    let guidance = first
        .messages
        .iter()
        .filter_map(|message| match message.content.as_ref() {
            Some(ChatMessageContent::Text(text)) if text.contains("<chat_file_tool_guidance>") => {
                Some(text)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(guidance.len(), 1);
    assert!(approx_token_count(guidance[0]) < MAX_CHAT_CONTEXT_ITEM_TOKENS);

    let legacy = Prompt {
        input: dedicated.input,
        tools: third_party_tools,
        chat_file_tool_mode: ChatFileToolMode::Legacy,
        dedicated_file_tools_enabled: true,
        ..Default::default()
    };
    let legacy = build_chat_completions_request(&legacy, &model_info(), None, None)
        .expect("legacy request with same-named tools");
    assert!(legacy.messages.iter().all(|message| {
        !matches!(
            message.content.as_ref(),
            Some(ChatMessageContent::Text(text)) if text.contains("<chat_file_tool_guidance>")
        )
    }));
}

#[test]
fn file_tool_modes_build_complete_expected_requests() {
    let apply_patch = ToolSpec::Freeform(FreeformTool {
        name: "apply_patch".to_string(),
        description: "patch description".to_string(),
        format: FreeformToolFormat {
            r#type: "grammar".to_string(),
            syntax: "lark".to_string(),
            definition: "start: /[\\s\\S]+/".to_string(),
        },
    });
    let dedicated_tools = ["read_file", "edit_file", "write_file"]
        .map(first_party_file_tool)
        .to_vec();
    let mut all_tools = dedicated_tools.clone();
    all_tools.push(apply_patch.clone());
    let mapped = |name, kind| chat_tool_name(None, name, kind);
    let read_name = mapped("read_file", ChatToolCallKind::Function);
    let edit_name = mapped("edit_file", ChatToolCallKind::Function);
    let write_name = mapped("write_file", ChatToolCallKind::Function);
    let apply_patch_name = mapped("apply_patch", ChatToolCallKind::Custom);
    let expected_function = |description: &str, mapped_name: &str| {
        json!({
            "type": "function",
            "function": {
                "name": mapped_name,
                "description": description,
                "parameters": {
                    "type": "object",
                    "properties": {"path": {"type": "string"}},
                    "required": ["path"],
                    "additionalProperties": false
                }
            }
        })
    };
    let dedicated_tool_json = vec![
        expected_function(
            "Read a bounded text file through Codex's filesystem layer.",
            &read_name,
        ),
        expected_function(
            "Replace exact text in a previously read text file.",
            &edit_name,
        ),
        expected_function(
            "Create or overwrite a bounded text file through Codex's filesystem layer.",
            &write_name,
        ),
    ];
    let apply_patch_json = json!({
        "type": "function",
        "function": {
            "name": apply_patch_name,
            "description": "patch description\n\nPass the raw lark grammar body in the `input` string.",
            "parameters": {
                "type": "object",
                "properties": {
                    "input": {
                        "type": "string",
                        "description": "Raw freeform tool input."
                    }
                },
                "required": ["input"],
                "additionalProperties": false
            }
        }
    });
    let tool_call_info = HashMap::from([
        (
            read_name.clone(),
            ApiChatToolCallInfo {
                name: "read_file".to_string(),
                namespace: None,
                kind: ApiChatToolCallKind::Function,
            },
        ),
        (
            edit_name.clone(),
            ApiChatToolCallInfo {
                name: "edit_file".to_string(),
                namespace: None,
                kind: ApiChatToolCallKind::Function,
            },
        ),
        (
            write_name.clone(),
            ApiChatToolCallInfo {
                name: "write_file".to_string(),
                namespace: None,
                kind: ApiChatToolCallKind::Function,
            },
        ),
        (
            apply_patch_name,
            ApiChatToolCallInfo {
                name: "apply_patch".to_string(),
                namespace: None,
                kind: ApiChatToolCallKind::Custom,
            },
        ),
    ]);
    let guidance = ChatFileToolGuidance::new(read_name, edit_name, write_name).render();
    let expected_request = |tools: Vec<Value>, include_guidance: bool| {
        let mut messages = vec![ChatMessage::text(
            ChatMessageRole::System,
            "system instructions".to_string(),
        )];
        if include_guidance {
            messages.push(ChatMessage::text(
                ChatMessageRole::Developer,
                guidance.clone(),
            ));
        }
        messages.push(ChatMessage::text(
            ChatMessageRole::User,
            "edit safely".to_string(),
        ));
        ChatCompletionsApiRequest {
            model: "chat-model".to_string(),
            messages,
            stream: true,
            stream_options: ChatStreamOptions {
                include_usage: true,
            },
            tools,
            tool_choice: Some("auto".to_string()),
            parallel_tool_calls: Some(true),
            reasoning_effort: None,
            service_tier: None,
            response_format: None,
            tool_call_info: tool_call_info.clone(),
        }
    };
    let build = |mode, tools, hidden_tools| {
        build_chat_completions_request(
            &Prompt {
                input: vec![message(
                    "user",
                    vec![ContentItem::InputText {
                        text: "edit safely".to_string(),
                    }],
                )],
                base_instructions: BaseInstructions {
                    text: "system instructions".to_string(),
                },
                tools,
                hidden_tools,
                chat_file_tool_mode: mode,
                dedicated_file_tools_enabled: true,
                parallel_tool_calls: true,
                ..Default::default()
            },
            &model_info(),
            None,
            None,
        )
        .expect("build file-tool mode request")
    };

    let mut legacy_tool_json = dedicated_tool_json.clone();
    legacy_tool_json.push(apply_patch_json.clone());
    assert_eq!(
        build(ChatFileToolMode::Legacy, all_tools.clone(), Vec::new()),
        expected_request(legacy_tool_json, false),
    );
    assert_eq!(
        build(
            ChatFileToolMode::Dedicated,
            dedicated_tools,
            vec![apply_patch],
        ),
        expected_request(dedicated_tool_json.clone(), true),
    );
    let mut dedicated_with_patch_json = dedicated_tool_json;
    dedicated_with_patch_json.push(apply_patch_json);
    assert_eq!(
        build(
            ChatFileToolMode::DedicatedWithApplyPatch,
            all_tools,
            Vec::new(),
        ),
        expected_request(dedicated_with_patch_json, true),
    );
}

#[test]
fn dedicated_mapped_names_are_stable_across_reordering_and_apply_patch_visibility() {
    let apply_patch = ToolSpec::Freeform(FreeformTool {
        name: "apply_patch".to_string(),
        description: "patch".to_string(),
        format: FreeformToolFormat {
            r#type: "grammar".to_string(),
            syntax: "lark".to_string(),
            definition: "start: /[\\s\\S]+/".to_string(),
        },
    });
    let build = |tools: Vec<ToolSpec>, mode, hidden_tools| {
        let prompt = Prompt {
            input: vec![message(
                "user",
                vec![ContentItem::InputText {
                    text: "edit safely".to_string(),
                }],
            )],
            tools,
            hidden_tools,
            chat_file_tool_mode: mode,
            dedicated_file_tools_enabled: true,
            ..Default::default()
        };
        serde_json::to_value(
            build_chat_completions_request(&prompt, &model_info(), None, None)
                .expect("dedicated request"),
        )
        .expect("serialize request")
    };
    let dedicated_names = ["read_file", "edit_file", "write_file"]
        .map(|name| chat_tool_name(None, name, ChatToolCallKind::Function));
    let ordered = ["read_file", "edit_file", "write_file"]
        .map(first_party_file_tool)
        .to_vec();
    let reversed = ["write_file", "edit_file", "read_file"]
        .map(first_party_file_tool)
        .to_vec();
    let hidden = build(
        ordered.clone(),
        ChatFileToolMode::Dedicated,
        vec![apply_patch.clone()],
    );
    let reordered = build(
        reversed,
        ChatFileToolMode::Dedicated,
        vec![apply_patch.clone()],
    );
    let mut visible_apply_patch = ordered;
    visible_apply_patch.push(apply_patch);
    let visible = build(
        visible_apply_patch,
        ChatFileToolMode::DedicatedWithApplyPatch,
        Vec::new(),
    );

    for request in [&hidden, &reordered, &visible] {
        let names = request["tools"]
            .as_array()
            .expect("tools")
            .iter()
            .filter_map(|tool| tool.pointer("/function/name").and_then(Value::as_str))
            .collect::<Vec<_>>();
        for expected in &dedicated_names {
            assert!(
                names.contains(&expected.as_str()),
                "missing {expected}: {names:?}"
            );
        }
        assert_eq!(request["tool_choice"], "auto");
    }
    let guidance = |request: &Value| {
        request["messages"]
            .as_array()
            .expect("messages")
            .iter()
            .find(|message| {
                message["role"] == "developer"
                    && message["content"]
                        .as_str()
                        .is_some_and(|text| text.contains("<chat_file_tool_guidance>"))
            })
            .cloned()
            .expect("guidance")
    };
    assert_eq!(guidance(&hidden), guidance(&reordered));
    assert_eq!(guidance(&hidden), guidance(&visible));
}
