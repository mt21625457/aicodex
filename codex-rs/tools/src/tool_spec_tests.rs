use super::ConfiguredToolSpec;
use super::ResponsesApiNamespace;
use super::ResponsesApiWebSearchFilters;
use super::ResponsesApiWebSearchUserLocation;
use super::ToolSpec;
use crate::AdditionalProperties;
use crate::FreeformTool;
use crate::FreeformToolFormat;
use crate::JsonSchema;
use crate::ResponsesApiNamespaceTool;
use crate::ResponsesApiTool;
use crate::claude_tool_name;
use crate::create_tools_json_for_claude_messages;
use crate::create_tools_json_for_responses_api;
use codex_protocol::config_types::WebSearchContextSize;
use codex_protocol::config_types::WebSearchFilters as ConfigWebSearchFilters;
use codex_protocol::config_types::WebSearchUserLocation as ConfigWebSearchUserLocation;
use codex_protocol::config_types::WebSearchUserLocationType;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::collections::BTreeMap;

#[test]
fn tool_spec_name_covers_all_variants() {
    assert_eq!(
        ToolSpec::Function(ResponsesApiTool {
            name: "lookup_order".to_string(),
            description: "Look up an order".to_string(),
            strict: false,
            defer_loading: None,
            parameters: JsonSchema::object(
                BTreeMap::new(),
                /*required*/ None,
                /*additional_properties*/ None
            ),
            output_schema: None,
        })
        .name(),
        "lookup_order"
    );
    assert_eq!(
        ToolSpec::Namespace(ResponsesApiNamespace {
            name: "mcp__demo__".to_string(),
            description: "Demo tools".to_string(),
            tools: Vec::new(),
        })
        .name(),
        "mcp__demo__"
    );
    assert_eq!(
        ToolSpec::ToolSearch {
            execution: "sync".to_string(),
            description: "Search for tools".to_string(),
            parameters: JsonSchema::object(
                BTreeMap::new(),
                /*required*/ None,
                /*additional_properties*/ None
            ),
        }
        .name(),
        "tool_search"
    );
    assert_eq!(ToolSpec::LocalShell {}.name(), "local_shell");
    assert_eq!(
        ToolSpec::ImageGeneration {
            output_format: "png".to_string(),
        }
        .name(),
        "image_generation"
    );
    assert_eq!(
        ToolSpec::WebSearch {
            external_web_access: Some(true),
            filters: None,
            user_location: None,
            search_context_size: None,
            search_content_types: None,
        }
        .name(),
        "web_search"
    );
    assert_eq!(
        ToolSpec::Freeform(FreeformTool {
            name: "exec".to_string(),
            description: "Run a command".to_string(),
            format: FreeformToolFormat {
                r#type: "grammar".to_string(),
                syntax: "lark".to_string(),
                definition: "start: \"exec\"".to_string(),
            },
        })
        .name(),
        "exec"
    );
}

#[test]
fn configured_tool_spec_name_delegates_to_tool_spec() {
    assert_eq!(
        ConfiguredToolSpec::new(
            ToolSpec::Function(ResponsesApiTool {
                name: "lookup_order".to_string(),
                description: "Look up an order".to_string(),
                strict: false,
                defer_loading: None,
                parameters: JsonSchema::object(
                    BTreeMap::new(),
                    /*required*/ None,
                    /*additional_properties*/ None
                ),
                output_schema: None,
            }),
            /*supports_parallel_tool_calls*/ true,
        )
        .name(),
        "lookup_order"
    );
}

#[test]
fn web_search_config_converts_to_responses_api_types() {
    assert_eq!(
        ResponsesApiWebSearchFilters::from(ConfigWebSearchFilters {
            allowed_domains: Some(vec!["example.com".to_string()]),
        }),
        ResponsesApiWebSearchFilters {
            allowed_domains: Some(vec!["example.com".to_string()]),
        }
    );
    assert_eq!(
        ResponsesApiWebSearchUserLocation::from(ConfigWebSearchUserLocation {
            r#type: WebSearchUserLocationType::Approximate,
            country: Some("US".to_string()),
            region: Some("California".to_string()),
            city: Some("San Francisco".to_string()),
            timezone: Some("America/Los_Angeles".to_string()),
        }),
        ResponsesApiWebSearchUserLocation {
            r#type: WebSearchUserLocationType::Approximate,
            country: Some("US".to_string()),
            region: Some("California".to_string()),
            city: Some("San Francisco".to_string()),
            timezone: Some("America/Los_Angeles".to_string()),
        }
    );
}

#[test]
fn create_tools_json_for_responses_api_includes_top_level_name() {
    assert_eq!(
        create_tools_json_for_responses_api(&[ToolSpec::Function(ResponsesApiTool {
            name: "demo".to_string(),
            description: "A demo tool".to_string(),
            strict: false,
            defer_loading: None,
            parameters: JsonSchema::object(
                BTreeMap::from([("foo".to_string(), JsonSchema::string(/*description*/ None),)]),
                /*required*/ None,
                /*additional_properties*/ None
            ),
            output_schema: None,
        })])
        .expect("serialize tools"),
        vec![json!({
            "type": "function",
            "name": "demo",
            "description": "A demo tool",
            "strict": false,
            "parameters": {
                "type": "object",
                "properties": {
                    "foo": { "type": "string" }
                },
            },
        })]
    );
}

#[test]
fn namespace_tool_spec_serializes_expected_wire_shape() {
    assert_eq!(
        serde_json::to_value(ToolSpec::Namespace(ResponsesApiNamespace {
            name: "mcp__demo__".to_string(),
            description: "Demo tools".to_string(),
            tools: vec![ResponsesApiNamespaceTool::Function(ResponsesApiTool {
                name: "lookup_order".to_string(),
                description: "Look up an order".to_string(),
                strict: false,
                defer_loading: None,
                parameters: JsonSchema::object(
                    BTreeMap::from([(
                        "order_id".to_string(),
                        JsonSchema::string(/*description*/ None),
                    )]),
                    /*required*/ None,
                    /*additional_properties*/ None,
                ),
                output_schema: None,
            })],
        }))
        .expect("serialize namespace tool"),
        json!({
            "type": "namespace",
            "name": "mcp__demo__",
            "description": "Demo tools",
            "tools": [
                {
                    "type": "function",
                    "name": "lookup_order",
                    "description": "Look up an order",
                    "strict": false,
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "order_id": { "type": "string" },
                        },
                    },
                },
            ],
        })
    );
}

#[test]
fn web_search_tool_spec_serializes_expected_wire_shape() {
    assert_eq!(
        serde_json::to_value(ToolSpec::WebSearch {
            external_web_access: Some(true),
            filters: Some(ResponsesApiWebSearchFilters {
                allowed_domains: Some(vec!["example.com".to_string()]),
            }),
            user_location: Some(ResponsesApiWebSearchUserLocation {
                r#type: WebSearchUserLocationType::Approximate,
                country: Some("US".to_string()),
                region: Some("California".to_string()),
                city: Some("San Francisco".to_string()),
                timezone: Some("America/Los_Angeles".to_string()),
            }),
            search_context_size: Some(WebSearchContextSize::High),
            search_content_types: Some(vec!["text".to_string(), "image".to_string()]),
        })
        .expect("serialize web_search"),
        json!({
            "type": "web_search",
            "external_web_access": true,
            "filters": {
                "allowed_domains": ["example.com"],
            },
            "user_location": {
                "type": "approximate",
                "country": "US",
                "region": "California",
                "city": "San Francisco",
                "timezone": "America/Los_Angeles",
            },
            "search_context_size": "high",
            "search_content_types": ["text", "image"],
        })
    );
}

#[test]
fn tool_search_tool_spec_serializes_expected_wire_shape() {
    assert_eq!(
        serde_json::to_value(ToolSpec::ToolSearch {
            execution: "sync".to_string(),
            description: "Search app tools".to_string(),
            parameters: JsonSchema::object(
                BTreeMap::from([(
                    "query".to_string(),
                    JsonSchema::string(Some("Tool search query".to_string()),),
                )]),
                Some(vec!["query".to_string()]),
                Some(AdditionalProperties::Boolean(false))
            ),
        })
        .expect("serialize tool_search"),
        json!({
            "type": "tool_search",
            "execution": "sync",
            "description": "Search app tools",
            "parameters": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Tool search query",
                    }
                },
                "required": ["query"],
                "additionalProperties": false,
            },
        })
    );
}

#[test]
fn create_tools_json_for_claude_messages_flattens_supported_tools() {
    let result = create_tools_json_for_claude_messages(&[
        ToolSpec::Function(ResponsesApiTool {
            name: "lookup_order".to_string(),
            description: "Look up an order".to_string(),
            strict: false,
            defer_loading: None,
            parameters: JsonSchema::object(
                BTreeMap::from([(
                    "order_id".to_string(),
                    JsonSchema::string(/*description*/ None),
                )]),
                Some(vec!["order_id".to_string()]),
                Some(AdditionalProperties::Boolean(false)),
            ),
            output_schema: None,
        }),
        ToolSpec::Namespace(ResponsesApiNamespace {
            name: "mcp__demo__".to_string(),
            description: "Demo tools".to_string(),
            tools: vec![ResponsesApiNamespaceTool::Function(ResponsesApiTool {
                name: "search".to_string(),
                description: "Search demo".to_string(),
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
        ToolSpec::ToolSearch {
            execution: "client".to_string(),
            description: "Search available tools".to_string(),
            parameters: JsonSchema::object(
                BTreeMap::from([(
                    "query".to_string(),
                    JsonSchema::string(/*description*/ None),
                )]),
                Some(vec!["query".to_string()]),
                Some(AdditionalProperties::Boolean(false)),
            ),
        },
        ToolSpec::LocalShell {},
        ToolSpec::Freeform(FreeformTool {
            name: "apply_patch".to_string(),
            description: "Apply a patch".to_string(),
            format: FreeformToolFormat {
                r#type: "grammar".to_string(),
                syntax: "lark".to_string(),
                definition: "start: \"patch\"".to_string(),
            },
        }),
    ])
    .expect("serialize claude tools");

    assert_eq!(
        result.tools,
        vec![
            json!({
                "name": "lookup_order",
                "description": "Look up an order",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "order_id": { "type": "string" },
                    },
                    "required": ["order_id"],
                    "additionalProperties": false,
                },
            }),
            json!({
                "name": "mcp__demo__search",
                "description": "Demo tools\n\nSearch demo",
                "input_schema": {
                    "type": "object",
                    "properties": {},
                },
            }),
            json!({
                "name": "tool_search",
                "description": "Search available tools",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string" },
                    },
                    "required": ["query"],
                    "additionalProperties": false,
                },
            }),
            json!({
                "name": "local_shell",
                "description": "Runs a local shell command and returns its output.",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "command": {
                            "type": "array",
                            "items": { "type": "string" },
                        },
                        "workdir": { "type": "string" },
                        "timeout_ms": { "type": "number" },
                        "sandbox_permissions": { "type": "string" },
                        "justification": { "type": "string" },
                        "prefix_rule": {
                            "type": "array",
                            "items": { "type": "string" },
                        },
                    },
                    "required": ["command"],
                    "additionalProperties": false,
                },
            }),
            json!({
                "name": "apply_patch",
                "description": "Apply a patch",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "input": {
                            "type": "string",
                            "description": "Raw freeform tool input.",
                        },
                    },
                    "required": ["input"],
                    "additionalProperties": false,
                },
            }),
        ]
    );
    assert_eq!(
        result.tool_call_info[1].namespace,
        Some("mcp__demo__".to_string())
    );
    assert_eq!(
        result.tool_call_info[2].kind,
        crate::ClaudeToolCallKind::ToolSearch
    );
    assert_eq!(
        result.tool_call_info[4].kind,
        crate::ClaudeToolCallKind::Custom
    );
}

#[test]
fn claude_tool_name_sanitizes_and_bounds_names() {
    assert_eq!(
        claude_tool_name(Some("test/server/"), "do.something"),
        "test_server_do_something"
    );

    let long = claude_tool_name(
        Some("mcp__very_long_server_name_that_will_exceed_the_anthropic_limit__"),
        "very_long_tool_name_that_will_also_exceed_the_limit",
    );
    assert!(long.len() <= 64);
}

#[test]
fn create_tools_json_for_claude_messages_deduplicates_colliding_names() {
    let result = create_tools_json_for_claude_messages(&[
        ToolSpec::Function(ResponsesApiTool {
            name: "a.b".to_string(),
            description: "Dot".to_string(),
            strict: false,
            defer_loading: None,
            parameters: JsonSchema::object(
                BTreeMap::new(),
                /*required*/ None,
                /*additional_properties*/ None,
            ),
            output_schema: None,
        }),
        ToolSpec::Function(ResponsesApiTool {
            name: "a/b".to_string(),
            description: "Slash".to_string(),
            strict: false,
            defer_loading: None,
            parameters: JsonSchema::object(
                BTreeMap::new(),
                /*required*/ None,
                /*additional_properties*/ None,
            ),
            output_schema: None,
        }),
    ])
    .expect("serialize claude tools");

    let names = result
        .tools
        .iter()
        .map(|tool| tool["name"].as_str().expect("tool name"))
        .collect::<Vec<_>>();

    assert_eq!(names[0], "a_b");
    assert_ne!(names[1], "a_b");
    assert!(names[1].starts_with("a_b_"));
    assert!(names[1].len() <= 64);
    assert_eq!(result.tool_call_info[1].claude_name, names[1]);
}
