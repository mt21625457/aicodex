use super::*;
use crate::common::ResponseCreateWsRequest;
use crate::common::ResponsesApiRequest;
use pretty_assertions::assert_eq;
use serde_json::json;

#[tokio::test]
async fn websocket_budget_checks_actual_frame_and_preserves_continuation_metadata() {
    let mut request = request("deepseek-flash");
    let frame = ResponsesWsRequest::ResponseCreate(ResponseCreateWsRequest {
        previous_response_id: Some("previous-response".into()),
        ..ResponseCreateWsRequest::from(&request)
    });
    let expected = serde_json::to_value(&frame).unwrap();
    let encoded = serialize_websocket_request(&frame, budget(/*limit*/ 4096))
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&encoded).unwrap(),
        expected
    );
    request.instructions = "x".repeat(4096);
    let frame = ResponsesWsRequest::ResponseCreate(ResponseCreateWsRequest::from(&request));
    let error = serialize_websocket_request(&frame, budget(/*limit*/ 4096))
        .await
        .unwrap_err();
    assert!(!crate::api_bridge::map_api_error(error).is_retryable());
    request.instructions = "x".repeat(4096);
    request.input = serde_json::from_value(json!([{"type":"message", "role":"user", "content":[
        {"type":"input_image", "image_url":"data:image/png;base64,private-invalid"}
    ]}]))
    .unwrap();
    let frame = ResponsesWsRequest::ResponseCreate(ResponseCreateWsRequest::from(&request));
    let error = serialize_websocket_request(&frame, budget(/*limit*/ 4096))
        .await
        .unwrap_err();
    assert!(!error.to_string().contains("private-invalid"));
}

#[tokio::test]
async fn mimo_and_minimax_websocket_frames_prepare_images_and_preserve_metadata() {
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    let mut png = STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGMQMgkDAAD4AJ3MaiF4AAAAAElFTkSuQmCC").unwrap();
    png.resize(2 * 1024 * 1024, 0);
    for model in ["mimo-v2.6-flash", "MiniMax-M3"] {
        let original = json!({"type":"response.create", "model":model, "instructions":"keep", "previous_response_id":"previous", "client_metadata":{"route":"keep"},
            "input":[{"type":"function_call_output", "call_id":"screen", "output":[{"type":"input_image", "image_url":format!("data:image/png;base64,{}", STANDARD.encode(&png))}]}],
            "tools":[], "tool_choice":"auto", "parallel_tool_calls":false, "store":false, "stream":true, "include":[]});
        let mut request = request(model);
        request.instructions = "keep".into();
        request.input = serde_json::from_value(original["input"].clone()).unwrap();
        request.client_metadata =
            Some(serde_json::from_value(original["client_metadata"].clone()).unwrap());
        let frame = ResponsesWsRequest::ResponseCreate(ResponseCreateWsRequest {
            previous_response_id: Some("previous".into()),
            ..ResponseCreateWsRequest::from(&request)
        });
        let mut expected = serde_json::to_value(&frame).unwrap();
        let encoded = serialize_websocket_request(&frame, budget(/*limit*/ 4096))
            .await
            .unwrap();
        assert!(encoded.len() < 4096);
        let actual: serde_json::Value = serde_json::from_str(&encoded).unwrap();
        expected["input"][0]["output"][0]["image_url"] =
            actual["input"][0]["output"][0]["image_url"].clone();
        assert_eq!(actual, expected);
    }
}

fn request(model: &str) -> ResponsesApiRequest {
    ResponsesApiRequest {
        model: model.into(),
        instructions: String::new(),
        input: Vec::new(),
        tools: None,
        tool_choice: "auto".into(),
        parallel_tool_calls: false,
        reasoning: None,
        max_output_tokens: None,
        store: false,
        stream: true,
        stream_options: None,
        include: Vec::new(),
        service_tier: None,
        prompt_cache_key: None,
        text: None,
        client_metadata: None,
        access_programs: None,
    }
}

pub(super) fn budget(limit: usize) -> crate::request_budget::RequestBudget {
    crate::request_budget::RequestBudget::for_provider(&crate::provider::Provider {
        name: "gateway".into(),
        base_url: "https://example.invalid/v1".into(),
        query_params: None,
        headers: http::HeaderMap::new(),
        retry: crate::provider::RetryConfig {
            max_attempts: 1,
            base_delay: std::time::Duration::from_millis(1),
            retry_429: false,
            retry_5xx: false,
            retry_transport: false,
        },
        stream_idle_timeout: std::time::Duration::from_secs(1),
        request_body_max_bytes: Some(limit),
    })
}
