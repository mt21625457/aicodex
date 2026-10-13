use super::*;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use pretty_assertions::assert_eq;
use serde_json::json;

fn image() -> Value {
    let mut png = STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGMQMgkDAAD4AJ3MaiF4AAAAAElFTkSuQmCC").unwrap();
    png.resize(8192, 0);
    json!({"type":"input_image", "detail":"original", "image_url":format!("data:image/png;base64,{}", STANDARD.encode(png))})
}

fn history(model: &str) -> Value {
    let image = image();
    json!({"model":model, "input":[
        {"type":"message", "role":"user", "content":[image]},
        {"type":"function_call", "call_id":"old-1", "name":"view_image", "arguments":"{\"path\":\"/tmp/第一页.png\"}"},
        {"type":"function_call_output", "call_id":"old-1", "output":[image, {"type":"input_text", "text":"第一页：标题遮挡已修复"}]},
        {"type":"message", "role":"assistant", "content":[{"type":"output_text", "text":"第一页检查完成。"}]},
        {"type":"custom_tool_call", "call_id":"old-2", "name":"inspect", "input":"page 2"},
        {"type":"custom_tool_call_output", "call_id":"old-2", "output":[image]},
        {"type":"message", "role":"assistant", "content":[{"type":"output_text", "text":"第二页检查完成。"}]},
        {"type":"function_call", "call_id":"old-3", "name":"view_image", "arguments":"{\"path\":\"/tmp/3.png\"}"},
        {"type":"function_call_output", "call_id":"old-3", "output":[image]},
        {"type":"reasoning", "summary":[{"type":"summary_text", "text":"Inspect the latest pages together."}]},
        {"type":"function_call", "call_id":"latest-1", "name":"view_image", "arguments":"{\"path\":\"/tmp/4.png\"}"},
        {"type":"function_call", "call_id":"latest-2", "name":"view_image", "arguments":"{\"path\":\"/tmp/5.png\"}"},
        {"type":"function_call_output", "call_id":"latest-2", "output":[image]},
        {"type":"function_call_output", "call_id":"latest-1", "output":[image]}
    ], "tools":[{"type":"function", "name":"view_image", "parameters":{"description":"保留工具 schema"}}]})
}

fn prepared(body: Value, limit: usize) -> Value {
    let encoded = prepare_sync(body, RequestBudget { route_bytes: limit }).unwrap();
    serde_json::from_slice(encoded.as_bytes()).unwrap()
}

#[test]
fn deepseek_pressure_omits_oldest_images_only_and_preserves_replay_context() {
    for model in [
        "deepseek-v4-flash",
        "gateway:deepseek-v4-flash",
        "provider/deepseek-v4-flash",
        " Gateway:DeepSeek-V4-Flash ",
    ] {
        let body = history(model);
        let before = encode(&body).unwrap().as_bytes().len();
        // The request already fits, but crosses the 80% early-warning threshold.
        let actual = prepared(body.clone(), before);
        let mut expected = body;
        for index in [2, 5] {
            assert_eq!(actual["input"][index]["output"][0]["type"], "input_text");
            assert!(
                actual["input"][index]["output"][0]["text"]
                    .as_str()
                    .unwrap()
                    .contains("no longer visible")
            );
            expected["input"][index]["output"][0] = actual["input"][index]["output"][0].clone();
        }
        // Includes user attachment, newer history, both parallel results, paths,
        // summaries, call IDs and schemas, with all original image bytes intact.
        assert_eq!(actual, expected);
        assert!(encode(&actual).unwrap().as_bytes().len() <= before - before.div_ceil(5));
    }
}

#[test]
fn small_deepseek_requests_and_other_models_are_unchanged() {
    let body = history("deepseek-v4-flash");
    let before = encode(&body).unwrap().as_bytes().len();
    assert_eq!(prepared(body.clone(), before * 2), body);
    for model in [
        "gpt-6",
        "gateway/gpt-6",
        "MiniMax-M3",
        "mimo-v2.6-pro",
        "deepseekish",
        "deepseek:gpt-6",
    ] {
        let body = history(model);
        let before = encode(&body).unwrap().as_bytes().len();
        assert_eq!(prepared(body.clone(), before), body);
    }
}

#[test]
fn exact_eighty_percent_threshold_preserves_history() {
    let mut body = history("deepseek-v4-flash");
    let before = encode(&body).unwrap().as_bytes().len();
    let mut notice = String::new();
    // Align the final JSON length to four bytes for an exact 4/5 boundary.
    body["instructions"] = json!(notice);
    while !encode(&body).unwrap().as_bytes().len().is_multiple_of(4) {
        notice.push('x');
        body["instructions"] = json!(notice);
    }
    let aligned = encode(&body).unwrap().as_bytes().len();
    assert!(aligned > before);
    assert_eq!(prepared(body.clone(), aligned / 4 * 5), body);
}

#[test]
fn ambiguous_single_batch_and_latest_image_batch_before_text_only_tools_are_protected() {
    let image = image();
    let body = json!({"model":"deepseek-v4-flash", "input":[
        {"type":"function_call_output", "call_id":"first", "output":[image]},
        {"type":"custom_tool_call_output", "call_id":"second", "output":[image]},
        {"type":"function_call", "call_id":"text", "name":"exec", "arguments":"{}"},
        {"type":"function_call_output", "call_id":"text", "output":"done"},
        {"role":"user", "content":[{"type":"input_text", "text":"continue"}]}
    ]});
    let before = encode(&body).unwrap().as_bytes().len();
    assert_eq!(prepared(body.clone(), before), body);
}

#[test]
fn parallel_calls_preserve_earlier_sibling_results_even_with_intervening_metadata() {
    let mut body = history("deepseek-v4-flash");
    body["input"]
        .as_array_mut()
        .unwrap()
        .insert(13, json!({"type":"reasoning", "summary":[]}));
    let before = encode(&body).unwrap().as_bytes().len();
    let actual = prepared(body.clone(), before);
    assert_eq!(actual["input"][12], body["input"][12]);
    assert_eq!(actual["input"][14], body["input"][14]);
    assert_eq!(actual["input"][2]["output"][0]["type"], "input_text");
}

#[test]
fn references_quoted_content_and_user_images_are_never_history_candidates() {
    let mut body = history("deepseek-v4-flash");
    let extra = vec![
        json!({"type":"input_image", "file_id":"file-keep"}),
        json!({"type":"input_image", "image_url":"https://example.invalid/keep.png"}),
        json!({"type":"input_text", "text":"{\"type\":\"input_image\",\"image_url\":\"data:image/png;base64,quoted\"}"}),
    ];
    body["input"][2]["output"]
        .as_array_mut()
        .unwrap()
        .extend(extra.clone());
    let before = encode(&body).unwrap().as_bytes().len();
    let actual = prepared(body.clone(), before);
    assert_eq!(actual["input"][0], body["input"][0]);
    assert_eq!(
        &actual["input"][2]["output"].as_array().unwrap()[2..],
        extra
    );
}

#[test]
fn oversized_request_can_fit_without_reencoding_protected_original_images() {
    let body = history("deepseek-v4-flash");
    let before = encode(&body).unwrap().as_bytes().len();
    let limit = before * 3 / 4;
    let actual = prepared(body.clone(), limit);
    assert!(encode(&actual).unwrap().as_bytes().len() <= limit);
    for index in [2, 5, 8] {
        assert_eq!(actual["input"][index]["output"][0]["type"], "input_text");
    }
    for index in [0, 12, 13] {
        assert_eq!(actual["input"][index], body["input"][index]);
    }
}

#[test]
fn history_omission_and_existing_lossless_preparation_share_exact_byte_accounting() {
    let body = history("deepseek-v4-flash");
    let actual = prepared(body, 4096);
    assert!(encode(&actual).unwrap().as_bytes().len() <= 4096);
    for index in [2, 5, 8] {
        assert_eq!(actual["input"][index]["output"][0]["type"], "input_text");
    }
    for (index, field) in [(0, "content"), (12, "output"), (13, "output")] {
        let retained = &actual["input"][index][field][0];
        assert_eq!(retained["type"], "input_image");
        assert_eq!(retained["detail"], "original");
        assert!(
            retained["image_url"]
                .as_str()
                .unwrap()
                .starts_with("data:image/png;base64,")
        );
    }
}

#[test]
fn no_history_relief_keeps_hard_limit_and_does_not_expose_image_data() {
    let body = json!({"model":"deepseek-v4-flash", "instructions":"x".repeat(4096), "input":[
        {"role":"user", "content":[image()]},
        {"type":"function_call_output", "call_id":"latest", "output":[image()]}
    ]});
    let error = prepare_sync(body, RequestBudget { route_bytes: 2048 })
        .unwrap_err()
        .to_string();
    assert!(error.contains("request was not sent"));
    assert!(!error.contains("base64"));
}

#[test]
fn latest_reference_images_release_older_inline_images() {
    for reference in [
        json!({"type":"input_image", "file_id":"file-latest"}),
        json!({"type":"input_image", "image_url":"https://example.invalid/latest.png"}),
    ] {
        let mut body = history("deepseek-v4-flash");
        for index in [12, 13] {
            body["input"][index]["output"][0] = reference.clone();
        }
        let actual = prepared(body.clone(), 4096);
        for index in [2, 5, 8] {
            assert_eq!(actual["input"][index]["output"][0]["type"], "input_text");
        }
        for index in [12, 13] {
            assert_eq!(actual["input"][index], body["input"][index]);
        }
    }
}

#[test]
fn standalone_host_input_is_not_treated_as_disposable_tool_history() {
    for call_id in [None, Some(Value::Null), Some(json!("")), Some(json!("  "))] {
        let mut delivery = json!({"type":"function_call_output", "name":"send_message_to_thread", "namespace":"codex_app", "output":[image()]});
        if let Some(call_id) = call_id {
            delivery["call_id"] = call_id;
        }
        let mut body = history("deepseek-v4-flash");
        body["input"].as_array_mut().unwrap().insert(1, delivery);
        let before = encode(&body).unwrap().as_bytes().len();
        let actual = prepared(body.clone(), before);
        assert_eq!(actual["input"][1]["output"][0]["type"], "input_image");
        assert_eq!(actual["input"][1], body["input"][1]);
        assert_eq!(actual["input"][3]["output"][0]["type"], "input_text");
    }
}

#[test]
fn standalone_image_delivery_does_not_displace_the_latest_tool_batch() {
    let mut body = history("deepseek-v4-flash");
    body["input"].as_array_mut().unwrap().push(json!({
        "type":"function_call_output", "name":"send_message_to_thread",
        "namespace":"codex_app", "output":[image()]
    }));
    let before = encode(&body).unwrap().as_bytes().len();
    let actual = prepared(body.clone(), before * 4 / 5);
    // The host message is new input, not a new completed tool batch. Both
    // parallel tool images must still be available for the next model response.
    for index in [12, 13, 14] {
        assert_eq!(actual["input"][index], body["input"][index]);
    }
}

#[test]
fn tiny_historical_images_are_not_replaced_by_larger_notices() {
    let mut body = history("deepseek-v4-flash");
    body["input"][2]["output"][0] =
        json!({"type":"input_image", "image_url":"data:image/png;base64,tiny"});
    let expected = body["input"][2].clone();
    let before = encode(&body).unwrap().as_bytes().len();
    assert_eq!(prepared(body, before)["input"][2], expected);
}
