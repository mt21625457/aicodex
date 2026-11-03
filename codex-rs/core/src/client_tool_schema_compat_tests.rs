use crate::client_common::Prompt;
use codex_models_manager::model_info::model_info_from_slug;
use codex_protocol::models::ResponseItem;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;

fn declarations(namespace: &str, parameter_type: &str, id: &str) -> Value {
    let mut value = json!({
        "type": "additional_tools", "role": "developer", "id": id,
        "tools": [{"type": "namespace", "name": namespace, "tools": [{
            "type": "function", "name": "wait_agent", "strict": false,
            "parameters": {"type": "object", "properties": {
                "timeout_ms": {"type": parameter_type, "description": "Timeout in milliseconds."}
            }, "additionalProperties": false}
        }]}]
    });
    if namespace == "multi_agent_v1" {
        value["tools"][0]["tools"][0]["parameters"]["properties"]["targets"] =
            json!({"type": "array", "items": {"type": "string"}});
        value["tools"][0]["tools"][0]["parameters"]["required"] = json!(["targets"]);
    }
    value
}

#[test]
fn resumed_request_repairs_all_legacy_declarations_without_mutating_history() {
    let model = model_info_from_slug("gpt-6-astra");
    // 真实顺序：旧声明、已提交输入、新声明、后续输入；保留所有项及其先后关系。
    for namespace in ["collaboration", "multi_agent_v1"] {
        let original = json!([
            declarations(namespace, "integer", "at_old"),
            {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "first"}]},
            declarations(namespace, "number", "at_current"),
            {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "second"}]},
            declarations(namespace, "integer", "at_other_old")
        ]);
        let prompt = Prompt {
            input: serde_json::from_value(original).unwrap(),
            ..Default::default()
        };
        let saved = prompt.input.clone();
        let request = prompt.get_formatted_input_for_request(&model);
        let serialized = serde_json::to_value(&request).unwrap();
        let mut expected = serde_json::to_value(&saved).unwrap();
        for index in [0, 4] {
            expected[index]["tools"][0]["tools"][0]["parameters"]["properties"]["timeout_ms"]["type"] =
                json!("number");
            assert_ne!(serialized[index]["id"], expected[index]["id"]);
            expected[index]["id"] = serialized[index]["id"].clone();
        }
        assert_eq!(serialized, expected);
        assert_eq!(prompt.input, saved);
        assert_eq!(request, prompt.get_formatted_input_for_request(&model));
        let mut second_pass = request.clone();
        super::normalize_historical_wait_schema(&mut second_pass);
        assert_eq!(second_pass, request);
        assert_ne!(serialized[0]["id"], serialized[4]["id"]);
    }
}

#[test]
fn compatibility_preserves_other_tools_unknown_schemas_and_tool_arguments() {
    let mut other_tool = declarations("collaboration", "integer", "at_other_tool");
    other_tool["tools"][0]["tools"][0]["name"] = json!("other_wait");
    let input: Vec<ResponseItem> = serde_json::from_value(json!([
        declarations("custom", "integer", "at_custom"),
        declarations("collaboration", "string", "at_unknown"),
        declarations("collaboration", "number", "at_current"),
        other_tool,
        {"type": "function_call", "call_id": "call_fixture", "name": "wait_agent", "namespace": "collaboration", "arguments": "{\"timeout_ms\":180000.0}"},
        {"type": "function_call_output", "call_id": "call_fixture", "output": "retained result"}
    ])).unwrap();
    let mut request = input.clone();
    super::normalize_historical_wait_schema(&mut request);
    assert_eq!(request, input);
}

#[test]
fn compatibility_handles_declarations_without_ids_and_missing_parameters() {
    let mut legacy = declarations("collaboration", "integer", "at_old");
    legacy.as_object_mut().unwrap().remove("id");
    let mut missing = declarations("collaboration", "integer", "at_missing");
    missing["tools"][0]["tools"][0]
        .as_object_mut()
        .unwrap()
        .remove("parameters");
    let original: Vec<ResponseItem> = serde_json::from_value(json!([legacy, missing])).unwrap();
    let mut request = original.clone();
    super::normalize_historical_wait_schema(&mut request);
    assert!(request[0].id().is_none());
    assert_eq!(request[1], original[1]);
    assert_eq!(
        serde_json::to_value(&request[0]).unwrap()["tools"][0]["tools"][0]["parameters"]["properties"]
            ["timeout_ms"]["type"],
        "number"
    );
}
