use super::*;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use pretty_assertions::assert_eq;
use serde_json::json;

fn padded_png() -> String {
    let mut bytes = STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGMQMgkDAAD4AJ3MaiF4AAAAAElFTkSuQmCC").unwrap();
    bytes.resize(2 * MIB, 0);
    format!("data:image/png;base64,{}", STANDARD.encode(bytes))
}

#[tokio::test]
async fn all_models_preserve_large_original_images_when_whole_request_fits() {
    for model in [
        "gpt-6",
        "deepseek-flash",
        "mimo-v2.6-pro",
        "MiniMax-M3",
        "custom",
    ] {
        let body = json!({"model":model, "input":[{"role":"user", "content":[{"type":"input_image", "detail":"original", "image_url":padded_png()}]}]});
        let expected = serde_json::to_vec(&body).unwrap();
        let result = prepare(
            body,
            RequestBudget {
                route_bytes: 8 * MIB,
            },
        )
        .await
        .unwrap();
        assert_eq!(result.as_bytes(), expected);
    }
}

#[tokio::test]
async fn constrained_route_prepares_full_history_and_tool_outputs_for_every_model() {
    for model in [
        "gpt-6",
        "deepseek-flash",
        "mimo:mimo-v2.6-pro",
        "minimax/MiniMax-M3",
        "custom",
    ] {
        let image = json!({"type":"input_image", "detail":"original", "image_url":padded_png()});
        let body = json!({"model":model, "input":[
            {"type":"message", "id":"old-user", "role":"user", "content":[image.clone()]},
            {"type":"function_call_output", "call_id":"screen", "output":[image.clone()]},
            {"type":"custom_tool_call_output", "call_id":"custom", "output":[image]},
            {"role":"user", "content":[{"type":"input_text", "text":"请继续"}]}],
            "tools":[{"type":"input_image", "image_url":"quoted-schema-unchanged"}]});
        let result = prepare(body.clone(), RequestBudget { route_bytes: 4096 })
            .await
            .unwrap();
        let mut actual: Value = serde_json::from_slice(result.as_bytes()).unwrap();
        assert!(result.as_bytes().len() <= 4096);
        let image = actual["input"][0]["content"][0]["image_url"]
            .as_str()
            .unwrap();
        assert!(image.starts_with("data:image/png;base64,"));
        for (item, field) in [(0, "content"), (1, "output"), (2, "output")] {
            actual["input"][item][field][0]["image_url"] =
                body["input"][item][field][0]["image_url"].clone();
        }
        assert_eq!(actual, body);
    }
}

#[test]
fn exact_utf8_budget_includes_envelope_tools_and_text() {
    let body = json!({"type":"response.create", "model":"gpt-6", "previous_response_id":"previous", "input":[], "instructions":"汉字", "tools":[{"description":"schema"}]});
    let exact = serde_json::to_vec(&body).unwrap();
    assert_eq!(
        prepare_sync(
            body.clone(),
            RequestBudget {
                route_bytes: exact.len()
            }
        )
        .unwrap()
        .as_bytes(),
        exact
    );
    let error = prepare_sync(
        body,
        RequestBudget {
            route_bytes: exact.len() - 1,
        },
    )
    .unwrap_err();
    assert!(error.to_string().contains("request was not sent"));
    assert!(!crate::api_bridge::map_api_error(error).is_retryable());
}

#[tokio::test]
async fn stops_compressing_as_soon_as_request_fits_and_leaves_small_images_identical() {
    let large = padded_png();
    let small = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGMQMgkDAAD4AJ3MaiF4AAAAAElFTkSuQmCC";
    let body = json!({"model":"gpt-6", "input":[{"role":"user", "content":[
        {"type":"input_image", "image_url":small},
        {"type":"input_image", "image_url":large},
        {"type":"input_image", "image_url":large}]}]});
    let result = prepare(
        body.clone(),
        RequestBudget {
            route_bytes: 3 * MIB,
        },
    )
    .await
    .unwrap();
    let mut actual: Value = serde_json::from_slice(result.as_bytes()).unwrap();
    actual["input"][0]["content"][1]["image_url"] = json!(large);
    assert_eq!(actual, body);
}

#[tokio::test]
async fn references_and_lookalike_schema_content_are_preserved() {
    let body = json!({"model":"deepseek-flash", "input":[{"role":"user", "content":[
        {"type":"input_image", "image_url":"https://example.invalid/image.png"},
        {"type":"input_image", "file_id":"file-reference"}]}], "tools":[{"image_url":"data:private-quoted-text"}]});
    assert_eq!(
        prepare(body.clone(), RequestBudget { route_bytes: 4096 })
            .await
            .unwrap()
            .as_bytes(),
        serde_json::to_vec(&body).unwrap()
    );
    assert!(
        prepare(body, RequestBudget { route_bytes: 1 })
            .await
            .unwrap_err()
            .to_string()
            .contains("request was not sent")
    );
}

#[test]
fn effective_route_budget_limits_gpt_and_model_budget_limits_deepseek() {
    let body = json!({"model":"gpt-6", "instructions":"x".repeat(41*MIB), "input":[]});
    assert!(
        prepare_sync(
            body.clone(),
            RequestBudget {
                route_bytes: 40 * MIB
            }
        )
        .is_err()
    );
    assert!(
        prepare_sync(
            body.clone(),
            RequestBudget {
                route_bytes: 42 * MIB
            }
        )
        .is_ok()
    );
    let mut switched = body;
    switched["model"] = json!("deepseek:deepseek-flash");
    assert!(
        prepare_sync(
            switched,
            RequestBudget {
                route_bytes: 42 * MIB
            }
        )
        .is_err()
    );
}

#[tokio::test]
async fn malformed_images_fail_privately_when_optimization_is_needed() {
    let body = json!({"model":"mimo-v2.6-pro", "input":[{"role":"user", "content":[{"type":"input_image", "image_url":"data:image/png;base64,private-invalid"}]}]});
    let error = prepare(body, RequestBudget { route_bytes: 1 })
        .await
        .unwrap_err();
    assert!(!error.to_string().contains("private-invalid"));
    assert!(!crate::api_bridge::map_api_error(error).is_retryable());
}

#[tokio::test]
async fn original_detail_is_never_lossily_compressed_to_force_a_fit() {
    let url = "data:image/png;base64,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";
    let mut body = json!({"model":"gpt-6", "input":[{"role":"user", "content":[{"type":"input_image", "detail":"original", "image_url":url}]}]});
    let error = prepare(body.clone(), RequestBudget { route_bytes: 3000 })
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Original-detail images remain lossless")
    );
    body["input"][0]["content"][0]["detail"] = json!("high");
    let encoded = prepare(body, RequestBudget { route_bytes: 3000 })
        .await
        .unwrap();
    assert!(encoded.as_bytes().len() <= 3000);
    let value: Value = serde_json::from_slice(encoded.as_bytes()).unwrap();
    assert!(
        value["input"][0]["content"][0]["image_url"]
            .as_str()
            .unwrap()
            .starts_with("data:image/jpeg;base64,")
    );
}

#[test]
fn direct_endpoint_and_configured_gateway_use_different_budgets() {
    let mut provider = Provider {
        name: "OpenAI".into(),
        base_url: "https://api.openai.com/v1".into(),
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
        request_body_max_bytes: None,
    };
    let body = json!({"model":"gpt-6", "instructions":"x".repeat(41*MIB), "input":[]});
    assert!(prepare_sync(body.clone(), RequestBudget::for_provider(&provider)).is_ok());
    for url in [
        "https://api.openai.com.proxy.invalid/v1",
        "https://example.invalid/v1",
        "http://api.openai.com/v1",
    ] {
        provider.base_url = url.into();
        assert!(prepare_sync(body.clone(), RequestBudget::for_provider(&provider)).is_err());
    }
    provider.request_body_max_bytes = Some(42 * MIB);
    assert!(prepare_sync(body, RequestBudget::for_provider(&provider)).is_ok());
}
