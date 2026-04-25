use codex_protocol::error::CodexErr;
use codex_protocol::error::Result;
use codex_protocol::models::FunctionCallOutputBody;
use codex_protocol::models::FunctionCallOutputContentItem;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::LocalShellAction;
use codex_protocol::models::LocalShellExecAction;
use codex_protocol::models::LocalShellStatus;
use codex_protocol::models::ResponseItem;
use codex_tools::ToolSpec;
use serde_json::Map;
use serde_json::Value;
use serde_json::json;
use std::collections::HashSet;

const TOOL_INPUT_FIELD: &str = "input";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AnthropicToolView {
    pub(crate) name: String,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ToolUseState {
    pub(crate) id: Option<String>,
    pub(crate) name: Option<String>,
    pub(crate) input: Option<Value>,
    pub(crate) partial_json: String,
    pub(crate) complete: bool,
    pub(crate) parse_error: Option<String>,
}

pub(crate) fn build_tool_views(tools: &[ToolSpec]) -> Vec<AnthropicToolView> {
    tools
        .iter()
        .map(|tool| AnthropicToolView {
            name: tool.name().to_string(),
        })
        .collect()
}

pub fn supports_tool_spec(tool: &ToolSpec) -> bool {
    matches!(
        tool,
        ToolSpec::Function(_)
            | ToolSpec::Freeform(_)
            | ToolSpec::LocalShell {}
            | ToolSpec::ToolSearch { .. }
            | ToolSpec::ImageGeneration { .. }
            | ToolSpec::WebSearch { .. }
    )
}

pub(crate) fn build_anthropic_tools(tools: &[ToolSpec]) -> Result<Vec<Value>> {
    let mut mapped_tools = Vec::new();
    for tool in tools {
        if let Some(tool) = tool_spec_to_anthropic_tool(tool)? {
            mapped_tools.push(tool);
        }
    }
    Ok(mapped_tools)
}

pub(crate) fn freeform_tool_names(tools: &[ToolSpec]) -> HashSet<String> {
    tools
        .iter()
        .filter_map(|tool| match tool {
            ToolSpec::Freeform(tool) => Some(tool.name.clone()),
            _ => None,
        })
        .collect()
}

pub(crate) fn anthropic_tool_use_message(
    name: String,
    call_id: String,
    input: Value,
) -> anthropic_sdk::types::messages::MessageParam {
    anthropic_sdk::types::messages::MessageParam {
        role: "assistant".to_string(),
        content: anthropic_sdk::types::messages::MessageContent::Blocks(vec![json!({
            "type": "tool_use",
            "id": call_id,
            "name": name,
            "input": input,
        })]),
    }
}

pub(crate) fn anthropic_tool_result_message(
    call_id: String,
    output: &FunctionCallOutputPayload,
    is_error: bool,
) -> Result<anthropic_sdk::types::messages::MessageParam> {
    let mut block = json!({
        "type": "tool_result",
        "tool_use_id": call_id,
        "content": anthropic_tool_result_content(output)?,
    });
    if is_error && let Some(object) = block.as_object_mut() {
        object.insert("is_error".to_string(), Value::Bool(true));
    }

    Ok(anthropic_sdk::types::messages::MessageParam {
        role: "user".to_string(),
        content: anthropic_sdk::types::messages::MessageContent::Blocks(vec![block]),
    })
}

pub(crate) fn tool_use_to_response_item(
    index: usize,
    tool: &ToolUseState,
    freeform_tool_names: &HashSet<String>,
) -> Result<ResponseItem> {
    let name = tool.name.clone().ok_or_else(|| {
        CodexErr::Stream(format!("anthropic tool_use {index} missing name"), None)
    })?;
    let call_id = tool
        .id
        .clone()
        .ok_or_else(|| CodexErr::Stream(format!("anthropic tool_use {index} missing id"), None))?;
    let input = tool_input_value(tool)?;

    if name == "local_shell" {
        return Ok(ResponseItem::LocalShellCall {
            id: None,
            call_id: Some(call_id),
            status: LocalShellStatus::InProgress,
            action: LocalShellAction::Exec(LocalShellExecAction {
                command: object_string_vec(&input, "command"),
                timeout_ms: object_u64(&input, "timeout_ms"),
                working_directory: object_string(&input, "workdir"),
                env: None,
                user: None,
            }),
        });
    }

    if name == "tool_search" {
        return Ok(ResponseItem::ToolSearchCall {
            id: None,
            call_id: Some(call_id),
            status: Some("in_progress".to_string()),
            execution: "client".to_string(),
            arguments: input,
        });
    }

    if freeform_tool_names.contains(&name) {
        let text = match input {
            Value::Object(mut object) => match object.remove(TOOL_INPUT_FIELD) {
                Some(Value::String(text)) => text,
                Some(value) => value.to_string(),
                None => Value::Object(object).to_string(),
            },
            value => value.to_string(),
        };
        return Ok(ResponseItem::CustomToolCall {
            id: None,
            status: None,
            call_id,
            name,
            input: text,
        });
    }

    Ok(ResponseItem::FunctionCall {
        id: None,
        name,
        namespace: None,
        arguments: serde_json::to_string(&input).unwrap_or_else(|_| "{}".to_string()),
        call_id,
    })
}

pub(crate) fn finalize_tool_use(tool: &mut ToolUseState) {
    tool.complete = true;
    if tool.partial_json.is_empty() {
        if tool.input.is_none() {
            tool.input = Some(Value::Object(Map::new()));
        }
        return;
    }

    match serde_json::from_str::<Value>(&tool.partial_json) {
        Ok(Value::Object(object)) => {
            tool.input = Some(merge_object_values(
                tool.input
                    .clone()
                    .unwrap_or_else(|| Value::Object(Map::new())),
                Value::Object(object),
            ));
        }
        Ok(other) => {
            tool.parse_error = Some(format!(
                "anthropic tool input must deserialize to an object, got {}",
                other
            ));
        }
        Err(err) => {
            tool.parse_error = Some(format!("failed to parse anthropic tool input JSON: {err}"));
        }
    }
}

pub(crate) fn parse_json_object_or_wrapped(input: &str) -> Value {
    match serde_json::from_str::<Value>(input) {
        Ok(Value::Object(object)) => Value::Object(object),
        Ok(Value::Null) => Value::Object(Map::new()),
        Ok(other) => {
            let mut object = Map::new();
            object.insert(TOOL_INPUT_FIELD.to_string(), other);
            Value::Object(object)
        }
        Err(_) => {
            let mut object = Map::new();
            object.insert(
                TOOL_INPUT_FIELD.to_string(),
                Value::String(input.to_string()),
            );
            Value::Object(object)
        }
    }
}

fn tool_spec_to_anthropic_tool(spec: &ToolSpec) -> Result<Option<Value>> {
    match spec {
        ToolSpec::Function(function_tool) => {
            let input_schema = serde_json::to_value(&function_tool.parameters).map_err(|err| {
                CodexErr::InvalidRequest(format!(
                    "failed to serialize anthropic tool schema for `{}`: {err}",
                    function_tool.name
                ))
            })?;
            let mut tool = json!({
                "name": function_tool.name,
                "description": function_tool.description,
                "input_schema": input_schema,
            });
            if function_tool.strict
                && let Some(object) = tool.as_object_mut()
            {
                object.insert("strict".to_string(), Value::Bool(true));
            }
            Ok(Some(tool))
        }
        ToolSpec::Freeform(tool) => Ok(Some(json!({
            "name": tool.name,
            "description": tool.description,
            "input_schema": {
                "type": "object",
                "properties": {
                    TOOL_INPUT_FIELD: {
                        "type": "string",
                        "description": "Raw freeform tool input."
                    }
                },
                "required": [TOOL_INPUT_FIELD],
                "additionalProperties": false
            }
        }))),
        ToolSpec::LocalShell {} => Ok(Some(json!({
            "name": "local_shell",
            "description": "Runs a local shell command and returns its output.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "command": { "type": "array", "items": { "type": "string" } },
                    "workdir": { "type": "string" },
                    "timeout_ms": { "type": "number" }
                },
                "required": ["command"],
                "additionalProperties": false
            }
        }))),
        ToolSpec::ToolSearch {
            execution,
            description,
            parameters,
        } => Ok(Some(json!({
            "name": "tool_search",
            "description": description,
            "input_schema": serde_json::to_value(parameters).map_err(|err| {
                CodexErr::InvalidRequest(format!(
                    "failed to serialize anthropic tool schema for `tool_search`: {err}"
                ))
            })?,
            "x-codex-execution": execution,
        }))),
        ToolSpec::WebSearch {
            filters,
            user_location,
            ..
        } => {
            let mut tool = json!({
                "type": "web_search_20250305",
                "name": "web_search",
                "max_uses": 5,
            });
            let object = tool.as_object_mut().ok_or_else(|| {
                CodexErr::InvalidRequest(
                    "failed to build anthropic web_search tool definition".to_string(),
                )
            })?;
            if let Some(filters) = filters
                && let Some(allowed_domains) = &filters.allowed_domains
            {
                object.insert(
                    "allowed_domains".to_string(),
                    serde_json::to_value(allowed_domains).map_err(|err| {
                        CodexErr::InvalidRequest(format!(
                            "failed to serialize anthropic web_search domains: {err}"
                        ))
                    })?,
                );
            }
            if let Some(user_location) = user_location {
                object.insert(
                    "user_location".to_string(),
                    serde_json::to_value(user_location).map_err(|err| {
                        CodexErr::InvalidRequest(format!(
                            "failed to serialize anthropic web_search location: {err}"
                        ))
                    })?,
                );
            }
            Ok(Some(tool))
        }
        ToolSpec::ImageGeneration { output_format } => Ok(Some(json!({
            "type": "image_generation",
            "name": "image_generation",
            "output_format": output_format,
        }))),
    }
}

fn tool_input_value(tool: &ToolUseState) -> Result<Value> {
    if !tool.complete {
        return Err(CodexErr::Stream(
            "anthropic tool_use block ended before content_block_stop".to_string(),
            None,
        ));
    }
    if let Some(parse_error) = &tool.parse_error {
        return Err(CodexErr::Stream(parse_error.clone(), None));
    }
    Ok(tool
        .input
        .clone()
        .unwrap_or_else(|| Value::Object(Map::new())))
}

fn merge_object_values(base: Value, overlay: Value) -> Value {
    match (base, overlay) {
        (Value::Object(mut base), Value::Object(overlay)) => {
            base.extend(overlay);
            Value::Object(base)
        }
        (_, overlay) => overlay,
    }
}

fn anthropic_tool_result_content(output: &FunctionCallOutputPayload) -> Result<Value> {
    match &output.body {
        FunctionCallOutputBody::Text(text) => Ok(Value::String(text.clone())),
        FunctionCallOutputBody::ContentItems(items) => Ok(Value::Array(
            items
                .iter()
                .map(tool_output_content_item_to_anthropic_block)
                .collect::<Result<Vec<_>>>()?,
        )),
    }
}

fn tool_output_content_item_to_anthropic_block(
    item: &FunctionCallOutputContentItem,
) -> Result<Value> {
    match item {
        FunctionCallOutputContentItem::InputText { text } => Ok(json!({
            "type": "text",
            "text": text,
        })),
        FunctionCallOutputContentItem::InputImage { image_url, .. } => {
            image_url_to_anthropic_block(image_url)
        }
    }
}

pub(crate) fn image_url_to_anthropic_block(image_url: &str) -> Result<Value> {
    if let Some((media_type, data)) = image_url.strip_prefix("data:").and_then(|rest| {
        let (header, data) = rest.split_once(',')?;
        let (media_type, encoding) = header.split_once(';')?;
        if encoding != "base64" {
            return None;
        }
        Some((media_type.to_string(), data.to_string()))
    }) {
        return Ok(json!({
            "type": "image",
            "source": {
                "type": "base64",
                "media_type": media_type,
                "data": data,
            }
        }));
    }

    if image_url.starts_with("http://") || image_url.starts_with("https://") {
        return Ok(json!({
            "type": "image",
            "source": {
                "type": "url",
                "url": image_url,
            }
        }));
    }

    Err(CodexErr::UnsupportedOperation(format!(
        "anthropic image block requires a data URL or http(s) URL, got `{image_url}`"
    )))
}

fn object_string(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn object_u64(value: &Value, key: &str) -> Option<u64> {
    value.get(key).and_then(Value::as_u64)
}

fn object_string_vec(value: &Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}
