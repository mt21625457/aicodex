use super::ConfiguredToolSpec;
use super::ResponsesApiNamespace;
use super::ResponsesApiWebSearchFilters;
use super::ResponsesApiWebSearchUserLocation;
use super::ToolSpec;
use crate::AdditionalProperties;
use crate::CommandToolOptions;
use crate::FreeformTool;
use crate::FreeformToolFormat;
use crate::JsonSchema;
use crate::ResponsesApiNamespaceTool;
use crate::ResponsesApiTool;
use crate::ShellToolOptions;
use crate::ViewImageToolOptions;
use crate::claude_tool_name;
use crate::create_close_agent_tool_v1;
use crate::create_exec_command_tool;
use crate::create_image_generation_tool;
use crate::create_request_permissions_tool;
use crate::create_request_user_input_tool;
use crate::create_shell_command_tool;
use crate::create_shell_tool;
use crate::create_tools_json_for_claude_messages;
use crate::create_tools_json_for_responses_api;
use crate::create_update_plan_tool;
use crate::create_view_image_tool;
use crate::create_web_search_tool;
use crate::create_write_stdin_tool;
use crate::dynamic_tool_to_loadable_tool_spec;
use codex_protocol::config_types::WebSearchContextSize;
use codex_protocol::config_types::WebSearchFilters as ConfigWebSearchFilters;
use codex_protocol::config_types::WebSearchMode;
use codex_protocol::config_types::WebSearchUserLocation as ConfigWebSearchUserLocation;
use codex_protocol::config_types::WebSearchUserLocationType;
use codex_protocol::dynamic_tools::DynamicToolSpec;
use codex_protocol::openai_models::WebSearchToolType;
use pretty_assertions::assert_eq;
use serde_json::Value;
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
            "local_shell",
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

    let local_shell = claude_tool(&result, "local_shell");
    assert_required_field(local_shell, "command");
    assert_property_description_contains(
        local_shell,
        "command",
        "Command and arguments to execute locally.",
    );
    assert_property_description_contains(
        local_shell,
        "workdir",
        "Optional working directory for the command.",
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
        result.tool_call_info[4].kind,
        crate::ClaudeToolCallKind::Custom
    );
}

#[test]
fn create_tools_json_for_claude_messages_preserves_representative_tool_contracts() {
    let result = create_tools_json_for_claude_messages(&[
        create_exec_command_tool(CommandToolOptions {
            allow_login_shell: true,
            exec_permission_approvals_enabled: true,
        }),
        create_write_stdin_tool(),
        create_shell_tool(ShellToolOptions {
            exec_permission_approvals_enabled: true,
        }),
        create_shell_command_tool(CommandToolOptions {
            allow_login_shell: true,
            exec_permission_approvals_enabled: true,
        }),
        ToolSpec::LocalShell {},
        create_request_permissions_tool("Request more permissions".to_string()),
        create_update_plan_tool(),
        create_request_user_input_tool("Ask the user for input".to_string()),
        create_view_image_tool(ViewImageToolOptions {
            can_request_original_image_detail: true,
        }),
        create_close_agent_tool_v1(),
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

    let local_shell = claude_tool(&result, "local_shell");
    assert_required_field(local_shell, "command");
    assert_property_description_contains(
        local_shell,
        "command",
        "Command and arguments to execute locally.",
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
    let dynamic_tool = dynamic_tool_to_loadable_tool_spec(&DynamicToolSpec {
        namespace: Some("codex_app".to_string()),
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
    })
    .expect("convert dynamic tool");

    let result = create_tools_json_for_claude_messages(&[ToolSpec::from(dynamic_tool)])
        .expect("serialize dynamic tool");

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
fn create_tools_json_for_claude_messages_omits_hosted_tools() {
    let web_search = create_web_search_tool(crate::WebSearchToolOptions {
        web_search_mode: Some(WebSearchMode::Live),
        web_search_config: None,
        web_search_tool_type: WebSearchToolType::Text,
    })
    .expect("web search tool");
    let result = create_tools_json_for_claude_messages(&[
        web_search,
        create_image_generation_tool("png"),
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
    assert_eq!(names, vec!["lookup_order"]);
    assert_eq!(result.tool_call_info.len(), 1);
    assert_eq!(result.tool_call_info[0].claude_name, "lookup_order");
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

fn assert_property_description_contains(tool: &Value, property: &str, expected: &str) {
    let description = tool["input_schema"]["properties"][property]["description"]
        .as_str()
        .unwrap_or_else(|| panic!("missing description for property {property} in {tool}"));
    assert!(
        description.contains(expected),
        "expected {property} description to contain {expected:?}, got {description:?}"
    );
}
