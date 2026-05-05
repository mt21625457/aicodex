use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;

use crate::function_tool::FunctionCallError;
use crate::session::tests::make_session_and_context;
use crate::tools::context::ToolPayload;
use codex_mcp::ToolInfo;
use codex_protocol::dynamic_tools::DynamicToolSpec;
use codex_protocol::models::ResponseItem;
use codex_tools::ClaudeToolCallKind;
use codex_tools::ResponsesApiNamespaceTool;
use codex_tools::ToolName;
use codex_tools::ToolSpec;
use codex_tools::create_tools_json_for_claude_messages;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;

use super::ToolCall;
use super::ToolRouter;
use super::ToolRouterParams;

#[tokio::test]
#[expect(
    clippy::await_holding_invalid_type,
    reason = "test builds a router from session-owned MCP manager state"
)]
async fn parallel_support_does_not_match_namespaced_local_tool_names() -> anyhow::Result<()> {
    let (session, turn) = make_session_and_context().await;
    let mcp_tools = session
        .services
        .mcp_connection_manager
        .read()
        .await
        .list_all_tools()
        .await;
    let router = ToolRouter::from_config(
        &turn.tools_config,
        ToolRouterParams {
            deferred_mcp_tools: None,
            mcp_tools: Some(mcp_tools),
            unavailable_called_tools: Vec::new(),
            parallel_mcp_server_names: HashSet::new(),
            discoverable_tools: None,
            dynamic_tools: turn.dynamic_tools.as_slice(),
        },
    );

    let parallel_tool_name = ["shell", "local_shell", "exec_command", "shell_command"]
        .into_iter()
        .find(|name| {
            router.tool_supports_parallel(&ToolCall {
                tool_name: ToolName::plain(*name),
                call_id: "call-parallel-tool".to_string(),
                payload: ToolPayload::Function {
                    arguments: "{}".to_string(),
                },
            })
        })
        .expect("test session should expose a parallel shell-like tool");

    assert!(!router.tool_supports_parallel(&ToolCall {
        tool_name: ToolName::namespaced("mcp__server__", parallel_tool_name),
        call_id: "call-namespaced-tool".to_string(),
        payload: ToolPayload::Function {
            arguments: "{}".to_string(),
        },
    }));

    Ok(())
}

#[tokio::test]
async fn build_tool_call_uses_namespace_for_registry_name() -> anyhow::Result<()> {
    let (session, _) = make_session_and_context().await;
    let session = Arc::new(session);
    let tool_name = "create_event".to_string();

    let call = ToolRouter::build_tool_call(
        &session,
        ResponseItem::FunctionCall {
            id: None,
            name: tool_name.clone(),
            namespace: Some("mcp__codex_apps__calendar".to_string()),
            arguments: "{}".to_string(),
            call_id: "call-namespace".to_string(),
        },
    )
    .await?
    .expect("function_call should produce a tool call");

    assert_eq!(
        call.tool_name,
        ToolName::namespaced("mcp__codex_apps__calendar", tool_name)
    );
    assert_eq!(call.call_id, "call-namespace");
    match call.payload {
        ToolPayload::Function { arguments } => {
            assert_eq!(arguments, "{}");
        }
        other => panic!("expected function payload, got {other:?}"),
    }

    Ok(())
}

#[tokio::test]
async fn build_tool_call_rejects_invalid_claude_custom_tool_input() -> anyhow::Result<()> {
    let (session, _) = make_session_and_context().await;
    let session = Arc::new(session);

    let err = ToolRouter::build_tool_call(
        &session,
        ResponseItem::CustomToolCall {
            id: Some("toolu-bad".to_string()),
            status: Some("invalid_claude_custom_tool_input: missing input string".to_string()),
            call_id: "toolu-bad".to_string(),
            name: "apply_patch".to_string(),
            input: String::new(),
        },
    )
    .await
    .expect_err("invalid Claude custom input should be returned to model");

    assert_eq!(
        err,
        FunctionCallError::RespondToModel("missing input string".to_string())
    );

    Ok(())
}

#[tokio::test]
async fn mcp_parallel_support_uses_exact_payload_server() -> anyhow::Result<()> {
    let (_, turn) = make_session_and_context().await;
    let router = ToolRouter::from_config(
        &turn.tools_config,
        ToolRouterParams {
            deferred_mcp_tools: None,
            mcp_tools: None,
            unavailable_called_tools: Vec::new(),
            parallel_mcp_server_names: HashSet::from(["echo".to_string()]),
            discoverable_tools: None,
            dynamic_tools: turn.dynamic_tools.as_slice(),
        },
    );

    let deferred_call = ToolCall {
        tool_name: ToolName::namespaced("mcp__echo__", "query_with_delay"),
        call_id: "call-deferred".to_string(),
        payload: ToolPayload::Mcp {
            server: "echo".to_string(),
            tool: "query_with_delay".to_string(),
            raw_arguments: "{}".to_string(),
        },
    };
    assert!(router.tool_supports_parallel(&deferred_call));

    let different_server_call = ToolCall {
        tool_name: ToolName::namespaced("mcp__hello_echo__", "query_with_delay"),
        call_id: "call-other-server".to_string(),
        payload: ToolPayload::Mcp {
            server: "hello_echo".to_string(),
            tool: "query_with_delay".to_string(),
            raw_arguments: "{}".to_string(),
        },
    };
    assert!(!router.tool_supports_parallel(&different_server_call));

    Ok(())
}

#[tokio::test]
async fn model_visible_specs_filter_deferred_dynamic_tools() -> anyhow::Result<()> {
    let (_, turn) = make_session_and_context().await;
    let hidden_tool = "hidden_dynamic_tool";
    let visible_tool = "visible_dynamic_tool";
    let dynamic_tools = vec![
        DynamicToolSpec {
            namespace: Some("codex_app".to_string()),
            name: hidden_tool.to_string(),
            description: "Hidden until discovered.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false,
            }),
            defer_loading: true,
        },
        DynamicToolSpec {
            namespace: Some("codex_app".to_string()),
            name: visible_tool.to_string(),
            description: "Visible immediately.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false,
            }),
            defer_loading: false,
        },
    ];

    let router = ToolRouter::from_config(
        &turn.tools_config,
        ToolRouterParams {
            deferred_mcp_tools: None,
            mcp_tools: None,
            unavailable_called_tools: Vec::new(),
            parallel_mcp_server_names: HashSet::new(),
            discoverable_tools: None,
            dynamic_tools: &dynamic_tools,
        },
    );

    assert!(
        router
            .find_spec(&ToolName::namespaced("codex_app", hidden_tool))
            .is_some()
    );
    assert_eq!(
        namespace_function_names(&router.specs(), "codex_app"),
        vec![hidden_tool.to_string(), visible_tool.to_string()]
    );
    assert_eq!(
        namespace_function_names(&router.model_visible_specs(), "codex_app"),
        vec![visible_tool.to_string()]
    );

    Ok(())
}

#[tokio::test]
async fn claude_model_visible_tools_have_registered_handlers() -> anyhow::Result<()> {
    let (_, turn) = make_session_and_context().await;
    let mut tools_config = turn.tools_config;
    tools_config.namespace_tools = true;
    tools_config.search_tool = true;

    let mcp_namespace = "mcp__test_server__";
    let mcp_tool_name = "echo";
    let visible_dynamic_tool = "visible_dynamic_tool";
    let deferred_dynamic_tool = "deferred_dynamic_tool";
    let dynamic_tools = vec![
        DynamicToolSpec {
            namespace: Some("codex_app".to_string()),
            name: visible_dynamic_tool.to_string(),
            description: "Visible dynamic tool.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false,
            }),
            defer_loading: false,
        },
        DynamicToolSpec {
            namespace: Some("codex_app".to_string()),
            name: deferred_dynamic_tool.to_string(),
            description: "Deferred dynamic tool.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false,
            }),
            defer_loading: true,
        },
    ];
    let mcp_tools = HashMap::from([(
        "mcp__test_server__echo".to_string(),
        mcp_tool_info(mcp_tool(
            mcp_tool_name,
            "Echo input.",
            json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false,
            }),
        )),
    )]);
    let router = ToolRouter::from_config(
        &tools_config,
        ToolRouterParams {
            deferred_mcp_tools: None,
            mcp_tools: Some(mcp_tools),
            unavailable_called_tools: Vec::new(),
            parallel_mcp_server_names: HashSet::new(),
            discoverable_tools: None,
            dynamic_tools: &dynamic_tools,
        },
    );

    let claude_tools = create_tools_json_for_claude_messages(&router.model_visible_specs())?;
    let claude_names = claude_tools
        .tools
        .iter()
        .map(claude_tool_name)
        .collect::<Vec<_>>();
    assert_eq!(claude_names.len(), claude_tools.tool_call_info.len());

    for claude_name in &claude_names {
        let mapping_count = claude_tools
            .tool_call_info
            .iter()
            .filter(|info| info.claude_name == *claude_name)
            .count();
        assert_eq!(
            mapping_count, 1,
            "Claude tool `{claude_name}` should have exactly one reverse mapping"
        );
    }

    for info in &claude_tools.tool_call_info {
        let tool_name = ToolName::new(info.namespace.clone(), info.name.clone());
        assert!(
            router.has_handler_for_test(&tool_name),
            "Claude tool `{}` maps to `{tool_name}` without a registered handler",
            info.claude_name
        );
        if info.kind == ClaudeToolCallKind::ToolSearch {
            assert_eq!(tool_name, ToolName::plain("tool_search"));
        }
    }

    assert!(
        claude_tools.tool_call_info.iter().any(|info| {
            info.namespace.as_deref() == Some(mcp_namespace) && info.name == mcp_tool_name
        }),
        "representative MCP tool should be advertised through Claude"
    );
    assert!(
        claude_tools.tool_call_info.iter().any(|info| {
            info.namespace.as_deref() == Some("codex_app") && info.name == visible_dynamic_tool
        }),
        "visible dynamic tool should be advertised through Claude"
    );
    assert!(
        claude_tools
            .tool_call_info
            .iter()
            .any(|info| info.kind == ClaudeToolCallKind::ToolSearch),
        "deferred dynamic tools should advertise the special tool_search path"
    );
    assert!(
        !claude_names.iter().any(|name| name == "web_search"),
        "Claude must not advertise hosted web_search"
    );
    assert!(
        !claude_names.iter().any(|name| name == "image_generation"),
        "Claude must not advertise hosted image_generation"
    );

    Ok(())
}

fn namespace_function_names(specs: &[ToolSpec], namespace_name: &str) -> Vec<String> {
    specs
        .iter()
        .find_map(|spec| match spec {
            ToolSpec::Namespace(namespace) if namespace.name == namespace_name => Some(
                namespace
                    .tools
                    .iter()
                    .map(|tool| match tool {
                        ResponsesApiNamespaceTool::Function(tool) => tool.name.clone(),
                    })
                    .collect(),
            ),
            ToolSpec::Function(_)
            | ToolSpec::Freeform(_)
            | ToolSpec::ToolSearch { .. }
            | ToolSpec::LocalShell {}
            | ToolSpec::ImageGeneration { .. }
            | ToolSpec::WebSearch { .. }
            | ToolSpec::Namespace(_) => None,
        })
        .unwrap_or_default()
}

fn claude_tool_name(tool: &Value) -> String {
    tool["name"]
        .as_str()
        .unwrap_or_else(|| panic!("Claude tool should have a name: {tool}"))
        .to_string()
}

fn mcp_tool(name: &str, description: &str, input_schema: Value) -> rmcp::model::Tool {
    rmcp::model::Tool {
        name: name.to_string().into(),
        title: None,
        description: Some(description.to_string().into()),
        input_schema: Arc::new(rmcp::model::object(input_schema)),
        output_schema: None,
        annotations: None,
        execution: None,
        icons: None,
        meta: None,
    }
}

fn mcp_tool_info(tool: rmcp::model::Tool) -> ToolInfo {
    ToolInfo {
        server_name: "test_server".to_string(),
        callable_name: tool.name.to_string(),
        callable_namespace: "mcp__test_server__".to_string(),
        server_instructions: None,
        tool,
        connector_id: None,
        connector_name: None,
        plugin_display_names: Vec::new(),
        connector_description: None,
    }
}
