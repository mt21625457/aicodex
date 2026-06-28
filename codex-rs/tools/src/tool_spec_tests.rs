use super::ClaudeMessagesToolOptions;
use super::ClaudeWebSearchToolKind;
use super::ResponsesApiNamespace;
use super::ResponsesApiWebSearchFilters;
use super::ResponsesApiWebSearchUserLocation;
use super::ToolSpec;
use crate::AdditionalProperties;
use crate::ClaudeBetaFeature;
use crate::ClaudeHistoryRequirements;
use crate::ClaudeLocalExecutorCapability;
use crate::ClaudeMcpServer;
use crate::ClaudeMcpToolsetConfig;
use crate::ClaudeNativeToolDecisionOutcome;
use crate::ClaudeNativeToolExecution;
use crate::ClaudeNativeToolKind;
use crate::ClaudeNativeToolSelection;
use crate::ClaudeProviderPlatform;
use crate::ClaudeServerCapability;
use crate::FreeformTool;
use crate::FreeformToolFormat;
use crate::JsonSchema;
use crate::ResponsesApiNamespaceTool;
use crate::ResponsesApiTool;
use crate::claude_tool_name;
use crate::create_tools_json_for_claude_messages;
use crate::create_tools_json_for_responses_api;
use crate::dynamic_tool_to_responses_api_tool;
use codex_protocol::config_types::WebSearchContextSize;
use codex_protocol::config_types::WebSearchFilters as ConfigWebSearchFilters;
use codex_protocol::config_types::WebSearchUserLocation as ConfigWebSearchUserLocation;
use codex_protocol::config_types::WebSearchUserLocationType;
use codex_protocol::dynamic_tools::DynamicToolFunctionSpec;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;
use std::collections::BTreeMap;
use std::collections::BTreeSet;

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
            index_gated_web_access: None,
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
            index_gated_web_access: None,
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

    let names = result
        .tools
        .iter()
        .map(|tool| tool["name"].as_str().expect("tool name"))
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        vec![
            "lookup_order",
            "mcp__demo__search",
            "tool_search",
            "apply_patch"
        ]
    );
    let mapped_names = result
        .tool_call_info
        .iter()
        .map(|info| info.claude_name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(mapped_names, names);
    assert_eq!(
        claude_tool(&result, "lookup_order")["input_schema"],
        json!({
            "type": "object",
            "properties": {
                "order_id": { "type": "string" },
            },
            "required": ["order_id"],
            "additionalProperties": false,
        })
    );
    assert_eq!(
        claude_tool(&result, "mcp__demo__search")["description"],
        "Demo tools\n\nSearch demo"
    );
    assert_eq!(
        claude_tool(&result, "tool_search")["input_schema"],
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string" },
            },
            "required": ["query"],
            "additionalProperties": false,
        })
    );

    let apply_patch = claude_tool(&result, "apply_patch");
    assert_required_field(apply_patch, "input");
    assert_tool_description_contains(
        apply_patch,
        "Claude must call this freeform tool through a JSON `tool_use.input` object",
    );
    assert_tool_description_contains(apply_patch, "*** Begin Patch");
    assert_tool_description_contains(apply_patch, "*** Add File: <path>");
    assert_tool_description_contains(apply_patch, "+Hello world");
    assert_property_description_contains(apply_patch, "input", "*** Begin Patch");
    assert_property_description_contains(apply_patch, "input", "*** Add File: <path>");
    assert_property_description_contains(apply_patch, "input", "start with `+`");
    assert_eq!(
        result.tool_call_info[1].namespace,
        Some("mcp__demo__".to_string())
    );
    assert_eq!(
        result.tool_call_info[2].kind,
        crate::ClaudeToolCallKind::ToolSearch
    );
    assert_eq!(
        result.tool_call_info[3].kind,
        crate::ClaudeToolCallKind::Custom
    );
}

#[test]
fn create_tools_json_for_claude_messages_preserves_representative_tool_contracts() {
    let result = create_tools_json_for_claude_messages(&[
        test_function_tool(
            "exec_command",
            "Runs a command in a PTY.",
            &["cmd"],
            &[
                ("cmd", "Shell command to execute."),
                ("sandbox_permissions", "Sandbox permissions"),
            ],
        ),
        test_function_tool(
            "write_stdin",
            "Writes characters to an existing unified exec session.",
            &["session_id"],
            &[(
                "session_id",
                "Identifier of the running unified exec session.",
            )],
        ),
        test_function_tool(
            "shell",
            "Runs a shell command and returns its output.",
            &["command"],
            &[("command", "The command to execute")],
        ),
        test_function_tool(
            "shell_command",
            "Runs a shell command and returns its output.",
            &["command"],
            &[(
                "command",
                "The shell script to execute in the user's default shell",
            )],
        ),
        test_function_tool(
            "request_permissions",
            "Request more permissions",
            &["permissions"],
            &[("permissions", "Permission profile to request.")],
        ),
        test_function_tool(
            "update_plan",
            "Updates the task plan.",
            &["plan"],
            &[("plan", "The list of steps")],
        ),
        test_function_tool(
            "request_user_input",
            "Ask the user for input.",
            &["questions"],
            &[("questions", "Questions to show the user")],
        ),
        test_function_tool(
            "view_image",
            "View a local image.",
            &["path"],
            &[("path", "Local filesystem path")],
        ),
        test_function_tool(
            "close_agent",
            "Close an agent.",
            &["target"],
            &[("target", "Agent id to close")],
        ),
        ToolSpec::ToolSearch {
            execution: "client".to_string(),
            description: "Search available tools".to_string(),
            parameters: JsonSchema::object(
                BTreeMap::from([(
                    "query".to_string(),
                    JsonSchema::string(Some("Search query".to_string())),
                )]),
                Some(vec!["query".to_string()]),
                Some(AdditionalProperties::Boolean(false)),
            ),
        },
    ])
    .expect("serialize claude tools");

    let exec_command = claude_tool(&result, "exec_command");
    assert_required_field(exec_command, "cmd");
    assert_property_description_contains(exec_command, "cmd", "Shell command to execute.");
    assert_property_description_contains(
        exec_command,
        "sandbox_permissions",
        "Sandbox permissions",
    );

    let write_stdin = claude_tool(&result, "write_stdin");
    assert_required_field(write_stdin, "session_id");
    assert_property_description_contains(
        write_stdin,
        "session_id",
        "Identifier of the running unified exec session.",
    );

    let shell = claude_tool(&result, "shell");
    assert_required_field(shell, "command");
    assert_property_description_contains(shell, "command", "The command to execute");

    let shell_command = claude_tool(&result, "shell_command");
    assert_required_field(shell_command, "command");
    assert_property_description_contains(
        shell_command,
        "command",
        "The shell script to execute in the user's default shell",
    );

    let request_permissions = claude_tool(&result, "request_permissions");
    assert_required_field(request_permissions, "permissions");

    let update_plan = claude_tool(&result, "update_plan");
    assert_required_field(update_plan, "plan");
    assert_property_description_contains(update_plan, "plan", "The list of steps");

    let request_user_input = claude_tool(&result, "request_user_input");
    assert_required_field(request_user_input, "questions");
    assert_property_description_contains(
        request_user_input,
        "questions",
        "Questions to show the user",
    );

    let view_image = claude_tool(&result, "view_image");
    assert_required_field(view_image, "path");
    assert_property_description_contains(view_image, "path", "Local filesystem path");

    let close_agent = claude_tool(&result, "close_agent");
    assert_required_field(close_agent, "target");
    assert_property_description_contains(close_agent, "target", "Agent id to close");

    let tool_search = claude_tool(&result, "tool_search");
    assert_required_field(tool_search, "query");
    assert_property_description_contains(tool_search, "query", "Search query");
    assert_eq!(
        result
            .tool_call_info
            .iter()
            .find(|info| info.claude_name == "tool_search")
            .expect("tool_search info")
            .kind,
        crate::ClaudeToolCallKind::ToolSearch
    );
}

#[test]
fn create_tools_json_for_claude_messages_preserves_dynamic_tool_schema() {
    let dynamic_tool = DynamicToolFunctionSpec {
        name: "lookup_order".to_string(),
        description: "Look up an order".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "order_id": {
                    "type": "string",
                    "description": "Order identifier"
                }
            },
            "required": ["order_id"],
            "additionalProperties": false
        }),
        defer_loading: false,
    };
    let dynamic_tool = ToolSpec::Namespace(ResponsesApiNamespace {
        name: "codex_app".to_string(),
        description: "Dynamic tools".to_string(),
        tools: vec![ResponsesApiNamespaceTool::Function(
            dynamic_tool_to_responses_api_tool(&dynamic_tool).expect("convert dynamic tool"),
        )],
    });

    let result =
        create_tools_json_for_claude_messages(&[dynamic_tool]).expect("serialize dynamic tool");

    let claude_name = claude_tool_name(Some("codex_app"), "lookup_order");
    let tool = claude_tool(&result, &claude_name);
    assert_required_field(tool, "order_id");
    assert_property_description_contains(tool, "order_id", "Order identifier");
    assert_eq!(
        result.tool_call_info,
        vec![crate::ClaudeToolCallInfo {
            claude_name,
            name: "lookup_order".to_string(),
            namespace: Some("codex_app".to_string()),
            kind: crate::ClaudeToolCallKind::Function,
        }]
    );
}

#[test]
fn create_tools_json_for_claude_messages_maps_web_search_and_omits_image_generation() {
    let result = create_tools_json_for_claude_messages(&[
        ToolSpec::WebSearch {
            external_web_access: Some(true),
            index_gated_web_access: None,
            filters: Some(ResponsesApiWebSearchFilters {
                allowed_domains: Some(vec![
                    "example.com".to_string(),
                    "docs.example.com".to_string(),
                ]),
            }),
            user_location: Some(ResponsesApiWebSearchUserLocation {
                r#type: WebSearchUserLocationType::Approximate,
                country: Some("US".to_string()),
                region: Some("California".to_string()),
                city: Some("San Francisco".to_string()),
                timezone: Some("America/Los_Angeles".to_string()),
            }),
            search_context_size: None,
            search_content_types: None,
        },
        ToolSpec::ImageGeneration {
            output_format: "png".to_string(),
        },
        ToolSpec::Function(ResponsesApiTool {
            name: "lookup_order".to_string(),
            description: "Look up an order".to_string(),
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
    assert_eq!(names, vec!["web_search", "lookup_order"]);
    assert_eq!(
        claude_tool(&result, "web_search"),
        &json!({
            "type": "web_search_20250305",
            "name": "web_search",
            "allowed_domains": ["example.com", "docs.example.com"],
            "user_location": {
                "type": "approximate",
                "country": "US",
                "region": "California",
                "city": "San Francisco",
                "timezone": "America/Los_Angeles"
            }
        })
    );
    assert_eq!(result.tool_call_info.len(), 1);
    assert_eq!(result.tool_call_info[0].claude_name, "lookup_order");
    assert!(result.mcp_servers.is_empty());
    assert!(result.beta_headers.is_empty());
    assert!(
        result
            .native_tool_policy
            .enabled_tools
            .contains(&ClaudeNativeToolKind::WebSearch20250305)
    );
    assert!(result.native_tool_policy.fallback_tools.is_empty());
    assert_eq!(
        result.native_tool_policy.decisions[0].outcome,
        ClaudeNativeToolDecisionOutcome::Enabled
    );
    assert_eq!(
        result.history_requirements,
        ClaudeHistoryRequirements {
            preserve_server_tool_results: true,
            preserve_mcp_tool_results: false,
            preserve_structured_citations: true,
        }
    );
}

#[test]
fn create_tools_json_for_claude_messages_falls_back_when_web_search_context_size_would_be_dropped()
{
    let result = create_tools_json_for_claude_messages(&[ToolSpec::WebSearch {
        external_web_access: Some(true),
        index_gated_web_access: None,
        filters: Some(ResponsesApiWebSearchFilters {
            allowed_domains: Some(vec!["example.com".to_string()]),
        }),
        user_location: None,
        search_context_size: Some(WebSearchContextSize::Medium),
        search_content_types: None,
    }])
    .expect("serialize claude tools");

    assert_eq!(result.tools.len(), 1);
    assert_openai_web_search_function_tool(claude_tool(&result, "web_search"));
    assert_eq!(
        result.tool_call_info,
        vec![crate::ClaudeToolCallInfo {
            claude_name: "web_search".to_string(),
            name: "web_search".to_string(),
            namespace: None,
            kind: crate::ClaudeToolCallKind::Function,
        }]
    );
    assert!(
        result
            .native_tool_policy
            .fallback_tools
            .contains(&ClaudeNativeToolKind::WebSearch20250305)
    );
    assert!(
        !result
            .native_tool_policy
            .enabled_tools
            .contains(&ClaudeNativeToolKind::WebSearch20250305)
    );
    assert!(result.native_tool_policy.decisions.iter().any(|decision| {
        decision.tool == ClaudeNativeToolKind::WebSearch20250305
            && decision.outcome == ClaudeNativeToolDecisionOutcome::Fallback
            && decision.reason.contains("cannot represent all configured")
    }));
    assert_eq!(
        result.history_requirements,
        ClaudeHistoryRequirements::default()
    );
}

#[test]
fn create_tools_json_for_claude_messages_falls_back_when_native_web_search_is_not_lossless() {
    for tool in [
        ToolSpec::WebSearch {
            external_web_access: Some(false),
            index_gated_web_access: None,
            filters: None,
            user_location: None,
            search_context_size: None,
            search_content_types: None,
        },
        ToolSpec::WebSearch {
            external_web_access: Some(true),
            index_gated_web_access: None,
            filters: None,
            user_location: None,
            search_context_size: None,
            search_content_types: Some(vec!["text".to_string(), "image".to_string()]),
        },
    ] {
        let result = create_tools_json_for_claude_messages(&[tool]).expect("serialize tools");

        assert_eq!(result.tools.len(), 1);
        assert_openai_web_search_function_tool(claude_tool(&result, "web_search"));
        assert_eq!(
            result.tool_call_info,
            vec![crate::ClaudeToolCallInfo {
                claude_name: "web_search".to_string(),
                name: "web_search".to_string(),
                namespace: None,
                kind: crate::ClaudeToolCallKind::Function,
            }]
        );
        assert!(
            result
                .native_tool_policy
                .fallback_tools
                .contains(&ClaudeNativeToolKind::WebSearch20250305)
        );
        assert!(result.native_tool_policy.disabled_tools.is_empty());
        assert_eq!(
            result.native_tool_policy.decisions[0].outcome,
            ClaudeNativeToolDecisionOutcome::Fallback
        );
        assert_eq!(
            result.history_requirements,
            ClaudeHistoryRequirements::default()
        );
    }
}

#[test]
fn create_tools_json_for_claude_messages_can_map_web_search_to_local_function() {
    let result = crate::create_tools_json_for_claude_messages_with_options(
        &[ToolSpec::WebSearch {
            external_web_access: Some(true),
            index_gated_web_access: None,
            filters: None,
            user_location: None,
            search_context_size: None,
            search_content_types: None,
        }],
        ClaudeMessagesToolOptions {
            web_search_tool_kind: ClaudeWebSearchToolKind::LocalFunctionTool,
            ..ClaudeMessagesToolOptions::default()
        },
    )
    .expect("serialize claude tools");

    assert_eq!(result.tools.len(), 1);
    assert_openai_web_search_function_tool(claude_tool(&result, "web_search"));
    assert_eq!(
        result.tool_call_info,
        vec![crate::ClaudeToolCallInfo {
            claude_name: "web_search".to_string(),
            name: "web_search".to_string(),
            namespace: None,
            kind: crate::ClaudeToolCallKind::Function,
        }]
    );
    assert!(
        result
            .native_tool_policy
            .fallback_tools
            .contains(&ClaudeNativeToolKind::WebSearch20250305)
    );
    assert_eq!(
        result.native_tool_policy.decisions[0].outcome,
        ClaudeNativeToolDecisionOutcome::Fallback
    );
    assert_eq!(
        result.history_requirements,
        ClaudeHistoryRequirements::default()
    );
}

#[test]
fn claude_native_tool_capability_table_records_current_tool_types() {
    assert_eq!(
        ClaudeNativeToolKind::WebSearch20250305.tool_type(),
        "web_search_20250305"
    );
    assert_eq!(
        ClaudeNativeToolKind::WebSearch20260209.tool_type(),
        "web_search_20260209"
    );
    assert_eq!(
        ClaudeNativeToolKind::WebFetch20260209.tool_type(),
        "web_fetch_20260209"
    );
    assert_eq!(
        ClaudeNativeToolKind::CodeExecution20260120.tool_type(),
        "code_execution_20260120"
    );
    assert_eq!(
        ClaudeNativeToolKind::Advisor20260301.tool_type(),
        "advisor_20260301"
    );
    assert_eq!(ClaudeNativeToolKind::McpToolset.tool_type(), "mcp_toolset");
    assert_eq!(
        ClaudeNativeToolKind::TextEditor20250728.execution(),
        ClaudeNativeToolExecution::Client
    );
    assert_eq!(
        ClaudeNativeToolKind::ToolSearchRegex20251119.execution(),
        ClaudeNativeToolExecution::Server
    );
    assert_eq!(
        ClaudeNativeToolKind::Advisor20260301
            .beta_feature()
            .map(ClaudeBetaFeature::header_value),
        Some("advisor-tool-2026-03-01")
    );
    assert_eq!(
        ClaudeNativeToolKind::McpToolset
            .beta_feature()
            .map(ClaudeBetaFeature::header_value),
        Some("mcp-client-2025-11-20")
    );
}

#[test]
fn create_tools_json_for_claude_messages_records_disabled_native_tool_policy() {
    let result = crate::create_tools_json_for_claude_messages_with_options(
        &[],
        ClaudeMessagesToolOptions {
            native_tool_selection: ClaudeNativeToolSelection {
                provider_platform: ClaudeProviderPlatform::DeepSeekCompatible,
                allowed_tools: BTreeSet::from([
                    ClaudeNativeToolKind::TextEditor20250728,
                    ClaudeNativeToolKind::McpToolset,
                    ClaudeNativeToolKind::WebSearch20260209,
                ]),
                server_dynamic_filtering: ClaudeServerCapability::Enabled,
                remote_mcp_connector: ClaudeServerCapability::Enabled,
                ..ClaudeNativeToolSelection::default()
            },
            ..ClaudeMessagesToolOptions::default()
        },
    )
    .expect("serialize claude tools");

    assert!(result.tools.is_empty());
    assert!(
        result
            .native_tool_policy
            .disabled_tools
            .contains(&ClaudeNativeToolKind::TextEditor20250728)
    );
    assert!(
        result
            .native_tool_policy
            .disabled_tools
            .contains(&ClaudeNativeToolKind::McpToolset)
    );
    assert!(
        result
            .native_tool_policy
            .disabled_tools
            .contains(&ClaudeNativeToolKind::WebSearch20260209)
    );
    assert!(result.native_tool_policy.decisions.iter().any(|decision| {
        decision.tool == ClaudeNativeToolKind::TextEditor20250728
            && decision.outcome == ClaudeNativeToolDecisionOutcome::Disabled
            && decision.reason.contains("provider platform")
    }));
    assert!(result.native_tool_policy.decisions.iter().any(|decision| {
        decision.tool == ClaudeNativeToolKind::McpToolset
            && decision.outcome == ClaudeNativeToolDecisionOutcome::Disabled
            && decision.reason.contains("requires Anthropic API")
    }));
}

#[test]
fn create_tools_json_for_claude_messages_emits_enabled_native_tools() {
    let result = crate::create_tools_json_for_claude_messages_with_options(
        &[],
        ClaudeMessagesToolOptions {
            native_tool_selection: ClaudeNativeToolSelection {
                provider_platform: ClaudeProviderPlatform::AnthropicApi,
                allowed_tools: BTreeSet::from([
                    ClaudeNativeToolKind::TextEditor20250728,
                    ClaudeNativeToolKind::Bash20250124,
                    ClaudeNativeToolKind::WebFetch20260209,
                    ClaudeNativeToolKind::Advisor20260301,
                ]),
                text_editor_executor: ClaudeLocalExecutorCapability::Available,
                bash_executor: ClaudeLocalExecutorCapability::Available,
                server_dynamic_filtering: ClaudeServerCapability::Enabled,
                advisor_model: Some("claude-opus-4-7".to_string()),
                ..ClaudeNativeToolSelection::default()
            },
            ..ClaudeMessagesToolOptions::default()
        },
    )
    .expect("serialize claude tools");

    assert_eq!(
        result.tools,
        vec![
            json!({
                "type": "web_fetch_20260209",
                "name": "web_fetch"
            }),
            json!({
                "type": "advisor_20260301",
                "name": "advisor",
                "model": "claude-opus-4-7"
            }),
            json!({
                "type": "bash_20250124",
                "name": "bash"
            }),
            json!({
                "type": "text_editor_20250728",
                "name": "str_replace_based_edit_tool",
                "max_characters": 20000
            }),
        ]
    );
    assert_eq!(
        result
            .beta_headers
            .iter()
            .copied()
            .map(ClaudeBetaFeature::header_value)
            .collect::<Vec<_>>(),
        vec!["advisor-tool-2026-03-01"]
    );
    assert!(result.history_requirements.preserve_server_tool_results);
    assert_eq!(
        result
            .tool_call_info
            .iter()
            .map(|info| info.claude_name.as_str())
            .collect::<Vec<_>>(),
        vec!["bash", "str_replace_based_edit_tool"]
    );
    assert!(result.native_tool_policy.disabled_tools.is_empty());
}

#[test]
fn create_tools_json_for_claude_messages_emits_native_remote_mcp_toolsets() {
    let result = crate::create_tools_json_for_claude_messages_with_options(
        &[],
        ClaudeMessagesToolOptions {
            native_tool_selection: ClaudeNativeToolSelection {
                provider_platform: ClaudeProviderPlatform::AnthropicApi,
                allowed_tools: BTreeSet::from([ClaudeNativeToolKind::McpToolset]),
                remote_mcp_connector: ClaudeServerCapability::Enabled,
                remote_mcp_servers: vec![ClaudeMcpServer {
                    name: "docs".to_string(),
                    url: "https://example.com/sse".to_string(),
                    authorization_token: Some("secret".to_string()),
                    toolset_config: ClaudeMcpToolsetConfig::default(),
                }],
                ..ClaudeNativeToolSelection::default()
            },
            ..ClaudeMessagesToolOptions::default()
        },
    )
    .expect("serialize claude tools");

    assert_eq!(
        result.tools,
        vec![json!({
            "type": "mcp_toolset",
            "mcp_server_name": "docs"
        })]
    );
    assert_eq!(
        result.mcp_servers,
        vec![ClaudeMcpServer {
            name: "docs".to_string(),
            url: "https://example.com/sse".to_string(),
            authorization_token: Some("secret".to_string()),
            toolset_config: ClaudeMcpToolsetConfig::default(),
        }]
    );
    assert_eq!(
        result
            .beta_headers
            .iter()
            .copied()
            .map(ClaudeBetaFeature::header_value)
            .collect::<Vec<_>>(),
        vec!["mcp-client-2025-11-20"]
    );
    assert!(result.history_requirements.preserve_mcp_tool_results);
    assert!(result.history_requirements.preserve_server_tool_results);
    assert!(
        result
            .native_tool_policy
            .enabled_tools
            .contains(&ClaudeNativeToolKind::McpToolset)
    );
}

#[test]
fn create_tools_json_for_claude_messages_emits_remote_mcp_toolset_configuration() {
    let result = crate::create_tools_json_for_claude_messages_with_options(
        &[],
        ClaudeMessagesToolOptions {
            native_tool_selection: ClaudeNativeToolSelection {
                provider_platform: ClaudeProviderPlatform::AnthropicApi,
                allowed_tools: BTreeSet::from([ClaudeNativeToolKind::McpToolset]),
                remote_mcp_connector: ClaudeServerCapability::Enabled,
                remote_mcp_servers: vec![ClaudeMcpServer {
                    name: "docs".to_string(),
                    url: "https://example.com/sse".to_string(),
                    authorization_token: None,
                    toolset_config: ClaudeMcpToolsetConfig {
                        default_config: Some(json!({"enabled": false})),
                        configs: Some(json!({
                            "search": {"enabled": true, "defer_loading": false}
                        })),
                        allowed_tools: Some(vec!["search".to_string()]),
                        denied_tools: Some(vec!["delete".to_string()]),
                        defer_loading: Some(true),
                        cache_control: Some(json!({"type": "ephemeral"})),
                    },
                }],
                ..ClaudeNativeToolSelection::default()
            },
            ..ClaudeMessagesToolOptions::default()
        },
    )
    .expect("serialize claude tools");

    assert_eq!(
        result.tools,
        vec![json!({
            "type": "mcp_toolset",
            "mcp_server_name": "docs",
            "default_config": {"enabled": false},
            "configs": {
                "search": {"enabled": true, "defer_loading": false}
            },
            "allowed_tools": ["search"],
            "denied_tools": ["delete"],
            "defer_loading": true,
            "cache_control": {"type": "ephemeral"}
        })]
    );
}

#[test]
fn create_tools_json_for_claude_messages_rejects_invalid_remote_mcp_servers() {
    let result = crate::create_tools_json_for_claude_messages_with_options(
        &[],
        ClaudeMessagesToolOptions {
            native_tool_selection: ClaudeNativeToolSelection {
                provider_platform: ClaudeProviderPlatform::AnthropicApi,
                allowed_tools: BTreeSet::from([ClaudeNativeToolKind::McpToolset]),
                remote_mcp_connector: ClaudeServerCapability::Enabled,
                remote_mcp_servers: vec![
                    ClaudeMcpServer {
                        name: "docs".to_string(),
                        url: "http://example.com/sse".to_string(),
                        authorization_token: None,
                        toolset_config: ClaudeMcpToolsetConfig::default(),
                    },
                    ClaudeMcpServer {
                        name: "docs".to_string(),
                        url: "https://example.com/other".to_string(),
                        authorization_token: None,
                        toolset_config: ClaudeMcpToolsetConfig::default(),
                    },
                ],
                ..ClaudeNativeToolSelection::default()
            },
            ..ClaudeMessagesToolOptions::default()
        },
    )
    .expect("serialize claude tools");

    assert!(result.tools.is_empty());
    assert!(result.mcp_servers.is_empty());
    assert!(result.beta_headers.is_empty());
    assert!(
        result
            .native_tool_policy
            .disabled_tools
            .contains(&ClaudeNativeToolKind::McpToolset)
    );
    assert!(result.native_tool_policy.decisions.iter().any(|decision| {
        decision.tool == ClaudeNativeToolKind::McpToolset
            && decision.reason.contains("unique HTTPS remote servers")
    }));
}

#[test]
fn create_tools_json_for_claude_messages_emits_native_tool_search_names() {
    let result = crate::create_tools_json_for_claude_messages_with_options(
        &[],
        ClaudeMessagesToolOptions {
            native_tool_selection: ClaudeNativeToolSelection {
                provider_platform: ClaudeProviderPlatform::AnthropicApi,
                model: Some("claude-sonnet-4-6".to_string()),
                allowed_tools: BTreeSet::from([
                    ClaudeNativeToolKind::ToolSearchRegex20251119,
                    ClaudeNativeToolKind::ToolSearchBm25V20251119,
                ]),
                tool_search: ClaudeServerCapability::Enabled,
                ..ClaudeNativeToolSelection::default()
            },
            ..ClaudeMessagesToolOptions::default()
        },
    )
    .expect("serialize claude tools");

    assert_eq!(
        result.tools,
        vec![
            json!({
                "type": "tool_search_tool_regex_20251119",
                "name": "tool_search_tool_regex"
            }),
            json!({
                "type": "tool_search_tool_bm25_20251119",
                "name": "tool_search_tool_bm25"
            }),
        ]
    );
    assert!(result.history_requirements.preserve_server_tool_results);
}

#[test]
fn create_tools_json_for_claude_messages_defers_large_catalog_with_native_tool_search() {
    let tools = (0..6)
        .map(|index| {
            test_function_tool(
                &format!("lookup_{index}"),
                "Lookup a catalog item",
                &["id"],
                &[("id", "Item id")],
            )
        })
        .collect::<Vec<_>>();
    let result = crate::create_tools_json_for_claude_messages_with_options(
        &tools,
        ClaudeMessagesToolOptions {
            native_tool_selection: ClaudeNativeToolSelection {
                provider_platform: ClaudeProviderPlatform::AnthropicApi,
                model: Some("claude-sonnet-4-6".to_string()),
                allowed_tools: BTreeSet::from([ClaudeNativeToolKind::ToolSearchRegex20251119]),
                tool_search: ClaudeServerCapability::Enabled,
                ..ClaudeNativeToolSelection::default()
            },
            ..ClaudeMessagesToolOptions::default()
        },
    )
    .expect("serialize claude tools");

    assert_ne!(
        claude_tool(&result, "tool_search_tool_regex")["defer_loading"].as_bool(),
        Some(true)
    );
    assert_ne!(
        claude_tool(&result, "lookup_0")["defer_loading"].as_bool(),
        Some(true)
    );
    assert_ne!(
        claude_tool(&result, "lookup_1")["defer_loading"].as_bool(),
        Some(true)
    );
    assert_ne!(
        claude_tool(&result, "lookup_2")["defer_loading"].as_bool(),
        Some(true)
    );
    assert_eq!(
        claude_tool(&result, "lookup_3")["defer_loading"].as_bool(),
        Some(true)
    );
    assert_eq!(
        claude_tool(&result, "lookup_5")["defer_loading"].as_bool(),
        Some(true)
    );
}

#[test]
fn claude_native_tool_planning_gates_by_model_platform_and_zdr_policy() {
    let result = crate::create_tools_json_for_claude_messages_with_options(
        &[],
        ClaudeMessagesToolOptions {
            native_tool_selection: ClaudeNativeToolSelection {
                provider_platform: ClaudeProviderPlatform::AnthropicApi,
                model: Some("claude-3-7-sonnet-20250219".to_string()),
                allowed_tools: BTreeSet::from([
                    ClaudeNativeToolKind::WebSearch20250305,
                    ClaudeNativeToolKind::WebSearch20260209,
                    ClaudeNativeToolKind::TextEditor20250728,
                    ClaudeNativeToolKind::TextEditor20250124,
                ]),
                require_zero_data_retention: true,
                text_editor_executor: ClaudeLocalExecutorCapability::Available,
                server_dynamic_filtering: ClaudeServerCapability::Enabled,
                ..ClaudeNativeToolSelection::default()
            },
            ..ClaudeMessagesToolOptions::default()
        },
    )
    .expect("serialize claude tools");

    assert_eq!(
        result.tools,
        vec![json!({
            "type": "text_editor_20250124",
            "name": "str_replace_based_edit_tool"
        })]
    );
    assert!(
        result
            .native_tool_policy
            .disabled_tools
            .contains(&ClaudeNativeToolKind::WebSearch20250305)
    );
    assert!(
        result
            .native_tool_policy
            .disabled_tools
            .contains(&ClaudeNativeToolKind::WebSearch20260209)
    );
    assert!(
        result
            .native_tool_policy
            .disabled_tools
            .contains(&ClaudeNativeToolKind::TextEditor20250728)
    );
    assert!(result.native_tool_policy.decisions.iter().any(|decision| {
        decision.tool == ClaudeNativeToolKind::WebSearch20250305
            && decision.reason.contains("zero data retention")
    }));
    assert!(result.native_tool_policy.decisions.iter().any(|decision| {
        decision.tool == ClaudeNativeToolKind::TextEditor20250728
            && decision.reason.contains("model does not support")
    }));
}

#[test]
fn claude_native_tool_planning_covers_platform_and_server_tool_versions() {
    let vertex = crate::create_tools_json_for_claude_messages_with_options(
        &[],
        ClaudeMessagesToolOptions {
            native_tool_selection: ClaudeNativeToolSelection {
                provider_platform: ClaudeProviderPlatform::Vertex,
                model: Some("claude-sonnet-4-6".to_string()),
                allowed_tools: BTreeSet::from([
                    ClaudeNativeToolKind::WebSearch20250305,
                    ClaudeNativeToolKind::WebFetch20260209,
                ]),
                server_dynamic_filtering: ClaudeServerCapability::Enabled,
                ..ClaudeNativeToolSelection::default()
            },
            ..ClaudeMessagesToolOptions::default()
        },
    )
    .expect("serialize claude tools");

    assert_eq!(
        vertex.tools,
        vec![json!({
            "type": "web_search_20250305",
            "name": "web_search"
        })]
    );
    assert!(
        vertex
            .native_tool_policy
            .disabled_tools
            .contains(&ClaudeNativeToolKind::WebFetch20260209)
    );

    let bedrock = crate::create_tools_json_for_claude_messages_with_options(
        &[],
        ClaudeMessagesToolOptions {
            native_tool_selection: ClaudeNativeToolSelection {
                provider_platform: ClaudeProviderPlatform::Bedrock,
                model: Some("claude-sonnet-4-6".to_string()),
                allowed_tools: BTreeSet::from([
                    ClaudeNativeToolKind::WebSearch20250305,
                    ClaudeNativeToolKind::CodeExecution20260120,
                ]),
                server_dynamic_filtering: ClaudeServerCapability::Enabled,
                ..ClaudeNativeToolSelection::default()
            },
            ..ClaudeMessagesToolOptions::default()
        },
    )
    .expect("serialize claude tools");

    assert!(bedrock.tools.is_empty());
    assert!(
        bedrock
            .native_tool_policy
            .disabled_tools
            .contains(&ClaudeNativeToolKind::WebSearch20250305)
    );
    assert!(
        bedrock
            .native_tool_policy
            .disabled_tools
            .contains(&ClaudeNativeToolKind::CodeExecution20260120)
    );
}

#[test]
fn claude_native_tool_planning_emits_newer_server_tools_when_allowed() {
    let result = crate::create_tools_json_for_claude_messages_with_options(
        &[],
        ClaudeMessagesToolOptions {
            native_tool_selection: ClaudeNativeToolSelection {
                provider_platform: ClaudeProviderPlatform::AnthropicApi,
                model: Some("claude-opus-4-7".to_string()),
                allowed_tools: BTreeSet::from([
                    ClaudeNativeToolKind::WebSearch20260209,
                    ClaudeNativeToolKind::WebFetch20260209,
                    ClaudeNativeToolKind::CodeExecution20260120,
                    ClaudeNativeToolKind::Advisor20260301,
                ]),
                server_dynamic_filtering: ClaudeServerCapability::Enabled,
                advisor_model: Some("claude-opus-4-7".to_string()),
                advisor_max_uses: Some(2),
                ..ClaudeNativeToolSelection::default()
            },
            ..ClaudeMessagesToolOptions::default()
        },
    )
    .expect("serialize claude tools");

    assert_eq!(
        result.tools,
        vec![
            json!({
                "type": "web_search_20260209",
                "name": "web_search"
            }),
            json!({
                "type": "web_fetch_20260209",
                "name": "web_fetch"
            }),
            json!({
                "type": "code_execution_20260120",
                "name": "code_execution"
            }),
            json!({
                "type": "advisor_20260301",
                "name": "advisor",
                "model": "claude-opus-4-7",
                "max_uses": 2
            }),
        ]
    );
    assert!(result.history_requirements.preserve_server_tool_results);
}

#[test]
fn claude_mcp_server_debug_redacts_authorization_token() {
    let server = ClaudeMcpServer {
        name: "docs".to_string(),
        url: "https://example.com/sse".to_string(),
        authorization_token: Some("secret-token".to_string()),
        toolset_config: ClaudeMcpToolsetConfig::default(),
    };

    let debug = format!("{server:?}");
    assert!(debug.contains("[redacted]"));
    assert!(!debug.contains("secret-token"));
}

#[test]
fn create_tools_json_for_claude_messages_exec_contains_code_mode_contract() {
    let result = create_tools_json_for_claude_messages(&[ToolSpec::Freeform(FreeformTool {
        name: "exec".to_string(),
        description: "Run JavaScript in Code Mode.".to_string(),
        format: FreeformToolFormat {
            r#type: "grammar".to_string(),
            syntax: "lark".to_string(),
            definition: r#"start: pragma_source | plain_source
pragma_source: PRAGMA_LINE NEWLINE SOURCE
plain_source: SOURCE
PRAGMA_LINE: /[ \t]*\/\/ @exec:[^\r\n]/
SOURCE: /[\s\S]+/"#
                .to_string(),
        },
    })])
    .expect("serialize claude tools");

    let exec = claude_tool(&result, "exec");
    assert_required_field(exec, "input");
    assert_tool_description_contains(
        exec,
        "Claude must call this freeform tool through a JSON `tool_use.input` object",
    );
    assert_tool_description_contains(exec, "PRAGMA_LINE");
    assert_property_description_contains(exec, "input", "Raw JavaScript source text");
    assert_property_description_contains(exec, "input", "not a Markdown code fence");
    assert_property_description_contains(exec, "input", "// @exec:");
}

#[test]
fn claude_tool_name_sanitizes_and_bounds_names() {
    assert_eq!(
        claude_tool_name(Some("test/server/"), "do.something"),
        "test_server_do_something"
    );
    assert_eq!(
        claude_tool_name(Some("codex_app"), "lookup_order"),
        "codex_app_lookup_order"
    );
    assert_eq!(
        claude_tool_name(Some("mcp__demo__"), "search"),
        "mcp__demo__search"
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

fn test_function_tool(
    name: &str,
    description: &str,
    required: &[&str],
    properties: &[(&str, &str)],
) -> ToolSpec {
    ToolSpec::Function(ResponsesApiTool {
        name: name.to_string(),
        description: description.to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(
            properties
                .iter()
                .map(|(name, description)| {
                    (
                        (*name).to_string(),
                        JsonSchema::string(Some((*description).to_string())),
                    )
                })
                .collect(),
            /*required*/ Some(required.iter().map(|field| (*field).to_string()).collect()),
            /*additional_properties*/ Some(false.into()),
        ),
        output_schema: None,
    })
}

fn claude_tool<'a>(result: &'a crate::ClaudeToolsJson, name: &str) -> &'a Value {
    result
        .tools
        .iter()
        .find(|tool| tool["name"].as_str() == Some(name))
        .unwrap_or_else(|| panic!("missing Claude tool {name}: {:?}", result.tools))
}

fn assert_required_field(tool: &Value, field: &str) {
    let required = tool["input_schema"]["required"]
        .as_array()
        .unwrap_or_else(|| panic!("missing required array in {tool}"));
    assert!(
        required.iter().any(|value| value.as_str() == Some(field)),
        "expected required field {field} in {tool}"
    );
}

fn assert_tool_description_contains(tool: &Value, expected: &str) {
    let description = tool["description"]
        .as_str()
        .unwrap_or_else(|| panic!("missing tool description in {tool}"));
    assert!(
        description.contains(expected),
        "expected tool description to contain {expected:?}, got {description:?}"
    );
}

fn assert_openai_web_search_function_tool(tool: &Value) {
    assert_eq!(tool["name"], json!("web_search"));
    assert_tool_description_contains(tool, "OpenAI web search command surface");
    assert_eq!(tool["input_schema"]["type"], json!("object"));
    assert_eq!(tool["input_schema"]["additionalProperties"], json!(false));

    let properties = tool["input_schema"]["properties"]
        .as_object()
        .unwrap_or_else(|| panic!("missing input_schema properties in {tool}"));
    for property in [
        "search_query",
        "image_query",
        "open",
        "click",
        "find",
        "screenshot",
        "finance",
        "weather",
        "sports",
        "time",
        "response_length",
        "query",
        "queries",
    ] {
        assert!(
            properties.contains_key(property),
            "expected web_search schema property {property} in {tool}"
        );
    }

    assert_eq!(
        tool["input_schema"]["properties"]["search_query"]["items"]["properties"]["q"]["type"],
        json!("string")
    );
    assert_eq!(
        tool["input_schema"]["properties"]["search_query"]["items"]["required"],
        json!(["q"])
    );
    assert_eq!(
        tool["input_schema"]["properties"]["open"]["items"]["properties"]["ref_id"]["type"],
        json!("string")
    );
    assert_eq!(
        tool["input_schema"]["properties"]["click"]["items"]["properties"]["id"]["type"],
        json!("integer")
    );
    assert_json_array_contains(
        &tool["input_schema"]["properties"]["find"]["items"]["required"],
        &["ref_id", "pattern"],
    );
    assert_eq!(
        tool["input_schema"]["properties"]["finance"]["items"]["properties"]["type"]["enum"],
        json!(["equity", "fund", "crypto", "index"])
    );
    assert_eq!(
        tool["input_schema"]["properties"]["weather"]["items"]["properties"]["location"]["type"],
        json!("string")
    );
    assert_eq!(
        tool["input_schema"]["properties"]["sports"]["items"]["properties"]["fn"]["enum"],
        json!(["schedule", "standings"])
    );
    assert_eq!(
        tool["input_schema"]["properties"]["time"]["items"]["properties"]["utc_offset"]["type"],
        json!("string")
    );
    assert_property_description_contains(tool, "query", "Legacy alias");
    assert_property_description_contains(tool, "queries", "Legacy alias");
}

fn assert_property_description_contains(tool: &Value, property: &str, expected: &str) {
    let description = tool["input_schema"]["properties"][property]["description"]
        .as_str()
        .unwrap_or_else(|| panic!("missing description for property {property} in {tool}"));
    assert!(
        description.contains(expected),
        "expected {property} description to contain {expected:?}, got {description:?}"
    );
}

fn assert_json_array_contains(value: &Value, expected: &[&str]) {
    let values = value
        .as_array()
        .unwrap_or_else(|| panic!("expected JSON array, got {value}"));
    for expected_item in expected {
        assert!(
            values
                .iter()
                .any(|value| value.as_str() == Some(*expected_item)),
            "expected JSON array {value} to contain {expected_item:?}"
        );
    }
}
