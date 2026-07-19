use super::*;
use crate::AdditionalProperties;
use crate::FreeformToolFormat;
use crate::JsonSchema;
use crate::ResponsesApiNamespace;
use pretty_assertions::assert_eq;
use std::collections::BTreeMap;

#[test]
fn serializes_chat_function_envelopes_and_reverse_metadata() {
    let lookup_name = chat_tool_name(
        /*namespace*/ None,
        "lookup",
        ChatToolCallKind::Function,
    );
    let namespaced_name = chat_tool_name(
        Some("codex.app"),
        "search/items",
        ChatToolCallKind::Function,
    );
    let apply_patch_name = chat_tool_name(
        /*namespace*/ None,
        "apply_patch",
        ChatToolCallKind::Custom,
    );
    let tool_search_name = chat_tool_name(
        /*namespace*/ None,
        "tool_search",
        ChatToolCallKind::ToolSearch,
    );
    let result = create_tools_json_for_chat_completions(&[
        ToolSpec::Function(ResponsesApiTool {
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
        }),
        ToolSpec::Namespace(ResponsesApiNamespace {
            name: "codex.app".to_string(),
            description: "App tools".to_string(),
            tools: vec![ResponsesApiNamespaceTool::Function(ResponsesApiTool {
                name: "search/items".to_string(),
                description: "Search items".to_string(),
                strict: false,
                defer_loading: None,
                parameters: JsonSchema::object(
                    BTreeMap::new(),
                    /*required*/ None,
                    /*additional_properties*/ None,
                ),
                output_schema: None,
            })],
        }),
        ToolSpec::Freeform(FreeformTool {
            name: "apply_patch".to_string(),
            description: "Apply a patch".to_string(),
            format: FreeformToolFormat {
                r#type: "grammar".to_string(),
                syntax: "lark".to_string(),
                definition: "start: patch".to_string(),
            },
        }),
        ToolSpec::ToolSearch {
            execution: "client".to_string(),
            description: "Search available tools".to_string(),
            parameters: JsonSchema::object(
                BTreeMap::new(),
                /*required*/ None,
                /*additional_properties*/ None,
            ),
        },
    ])
    .expect("serialize Chat tools");

    assert_eq!(
        result.tools,
        vec![
            json!({
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
            }),
            json!({
                "type": "function",
                "function": {
                    "name": namespaced_name,
                    "description": "App tools\n\nSearch items",
                    "parameters": {"type": "object", "properties": {}}
                }
            }),
            json!({
                "type": "function",
                "function": {
                    "name": apply_patch_name,
                    "description": "Apply a patch\n\nPass the raw lark grammar body in the `input` string.",
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
            }),
            json!({
                "type": "function",
                "function": {
                    "name": tool_search_name,
                    "description": "Search available tools",
                    "parameters": {"type": "object", "properties": {}}
                }
            }),
        ]
    );
    assert_eq!(
        result.tool_call_info,
        vec![
            ChatToolCallInfo {
                chat_name: lookup_name,
                name: "lookup".to_string(),
                namespace: None,
                kind: ChatToolCallKind::Function,
            },
            ChatToolCallInfo {
                chat_name: namespaced_name,
                name: "search/items".to_string(),
                namespace: Some("codex.app".to_string()),
                kind: ChatToolCallKind::Function,
            },
            ChatToolCallInfo {
                chat_name: apply_patch_name,
                name: "apply_patch".to_string(),
                namespace: None,
                kind: ChatToolCallKind::Custom,
            },
            ChatToolCallInfo {
                chat_name: tool_search_name,
                name: "tool_search".to_string(),
                namespace: None,
                kind: ChatToolCallKind::ToolSearch,
            },
        ]
    );
}

#[test]
fn chat_tool_names_are_stable_across_collisions_and_reordering() {
    let first = chat_tool_name(
        /*namespace*/ None,
        "same/name",
        ChatToolCallKind::Function,
    );
    let second = chat_tool_name(
        /*namespace*/ None,
        "same.name",
        ChatToolCallKind::Function,
    );

    assert_ne!(first, second);
    assert_eq!(
        first,
        chat_tool_name(
            /*namespace*/ None,
            "same/name",
            ChatToolCallKind::Function
        )
    );
    assert!(first.len() <= MAX_CHAT_TOOL_NAME_LEN);
    assert!(second.len() <= MAX_CHAT_TOOL_NAME_LEN);
}

#[test]
fn preserves_strict_function_schema_on_chat_wire() {
    let result = create_tools_json_for_chat_completions(&[ToolSpec::Function(ResponsesApiTool {
        name: "strict_lookup".to_string(),
        description: "Look up a value with strict arguments".to_string(),
        strict: true,
        defer_loading: None,
        parameters: JsonSchema::object(
            BTreeMap::from([("key".to_string(), JsonSchema::string(/*description*/ None))]),
            Some(vec!["key".to_string()]),
            Some(AdditionalProperties::Boolean(false)),
        ),
        output_schema: None,
    })])
    .expect("serialize strict Chat tool");

    assert_eq!(result.tools[0]["function"]["strict"], true);
}

#[test]
fn caps_chat_tools_at_the_provider_limit() {
    let tools = (0..=MAX_CHAT_TOOLS)
        .map(|index| function_tool(format!("tool_{index}"), String::new()))
        .collect::<Vec<_>>();

    let result = create_tools_json_for_chat_completions(&tools)
        .expect("serialize tools up to the Chat provider limit");

    assert_eq!(result.tools.len(), MAX_CHAT_TOOLS);
    assert_eq!(
        result
            .tool_call_info
            .iter()
            .map(|info| info.name.clone())
            .collect::<Vec<_>>(),
        (0..MAX_CHAT_TOOLS)
            .map(|index| format!("tool_{index}"))
            .collect::<Vec<_>>()
    );
}

#[test]
fn preserves_uncapped_reverse_metadata_for_hidden_tools() {
    let tools = (0..=MAX_CHAT_TOOLS)
        .map(|index| function_tool(format!("tool_{index}"), String::new()))
        .collect::<Vec<_>>();

    let tool_call_info = create_tool_call_info_for_chat_completions(&tools)
        .expect("build hidden Chat tool metadata");

    assert_eq!(tool_call_info.len(), MAX_CHAT_TOOLS + 1);
    assert_eq!(
        tool_call_info.last().map(|info| info.name.as_str()),
        Some("tool_128")
    );
}

fn function_tool(name: String, description: String) -> ToolSpec {
    ToolSpec::Function(ResponsesApiTool {
        name,
        description,
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(
            BTreeMap::new(),
            /*required*/ None,
            /*additional_properties*/ None,
        ),
        output_schema: None,
    })
}

#[test]
fn rejects_a_chat_tool_that_exceeds_its_context_budget() {
    let tool = function_tool(
        "oversized".to_string(),
        "x".repeat(approx_bytes_for_tokens(MAX_CHAT_TOOL_TOKENS) + 1),
    );

    let error = create_tools_json_for_chat_completions(&[tool])
        .expect_err("oversized tool should be rejected");

    assert!(error.to_string().contains("exceeds the 10000-token"));
}

#[test]
fn rejects_chat_tools_that_exceed_the_total_context_budget() {
    const TOOL_COUNT: usize = 72;
    let tokens_per_tool = MAX_CHAT_TOOLS_TOTAL_TOKENS / TOOL_COUNT + 100;
    let tools = (0..TOOL_COUNT)
        .map(|index| {
            function_tool(
                format!("tool_{index}"),
                "x".repeat(approx_bytes_for_tokens(tokens_per_tool)),
            )
        })
        .collect::<Vec<_>>();

    let error = create_tools_json_for_chat_completions(&tools)
        .expect_err("oversized tool set should be rejected");

    assert!(error.to_string().contains("64000-token total"));
}
