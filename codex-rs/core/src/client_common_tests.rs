use codex_api::OpenAiVerbosity;
use codex_api::ResponsesApiRequest;
use codex_api::TextControls;
use codex_api::create_text_param_for_request;
use codex_models_manager::model_info::model_info_from_slug;
use codex_protocol::config_types::ServiceTier;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::ImageDetail;
use codex_protocol::models::ImageReference;
use pretty_assertions::assert_eq;
use serde_json::value::RawValue;
use std::sync::Arc;

use super::*;

// 保留优化前的格式化算法作为差分基准，确保大正文和边界头部的输出逐字节不变。
fn structured_output_before_borrowing(parsed: &ExecOutputJson) -> String {
    let mut sections = vec![
        format!("Exit code: {}", parsed.metadata.exit_code),
        format!("Wall time: {} seconds", parsed.metadata.duration_seconds),
    ];
    let mut output = parsed.output.clone();
    if let Some((stripped, total_lines)) = strip_total_output_header(&parsed.output) {
        sections.push(format!("Total output lines: {total_lines}"));
        output = stripped.to_string();
    }
    sections.push("Output:".to_string());
    sections.push(output);
    sections.join("\n")
}

#[test]
fn structured_shell_output_matches_previous_format_for_boundary_and_large_outputs() {
    let mut outputs = [
        "",
        "普通输出🦀\n第二行\r\n",
        "Total output lines: 0\n",
        "Total output lines: 2\nfirst\nsecond\n",
        "Total output lines: 2\n\nfirst\nsecond\n",
        "Total output lines: 2\n\n\nfirst\nsecond\n",
        "Total output lines: 4294967295\nlast",
        "Total output lines: 4294967296\nunchanged",
        "Total output lines: invalid\nunchanged",
        "Total output lines: 2",
        "Total output lines: 2\r\nunchanged",
        "prefix\nTotal output lines: 2\nunchanged",
    ]
    .map(str::to_owned)
    .to_vec();
    let large_output = "日志🦀\n".repeat(128 * 1024);
    outputs.push(large_output.clone());
    outputs.push(format!("Total output lines: 131072\n\n{large_output}"));

    for output in outputs {
        for (exit_code, duration_seconds) in [
            (0, 0.0),
            (-1, -0.0),
            (i32::MAX, f32::MAX),
            (i32::MIN, f32::MIN_POSITIVE),
            (1, 1.2345),
            (1, f32::INFINITY),
            (1, f32::NAN),
        ] {
            let parsed = ExecOutputJson {
                output: output.clone(),
                metadata: ExecOutputMetadataJson {
                    exit_code,
                    duration_seconds,
                },
            };
            assert_eq!(
                build_structured_output(&parsed),
                structured_output_before_borrowing(&parsed),
                "output length={}, exit_code={exit_code}, duration={duration_seconds}",
                output.len()
            );
        }
    }
}

#[test]
fn structured_shell_output_preserves_json_validation_and_header_semantics() {
    for invalid in [
        "not json",
        r#"{"output":"ok"}"#,
        r#"{"output":null,"metadata":{"exit_code":0,"duration_seconds":1}}"#,
        r#"{"output":"ok","metadata":{"exit_code":"0","duration_seconds":1}}"#,
    ] {
        assert_eq!(parse_structured_shell_output(invalid), None);
    }
    assert_eq!(
        parse_structured_shell_output(
            r#"{"output":"Total output lines: 2\n\n一\n二\n","metadata":{"exit_code":-1,"duration_seconds":0.5}}"#
        ),
        Some(
            "Exit code: -1\nWall time: 0.5 seconds\nTotal output lines: 2\nOutput:\n一\n二\n"
                .to_owned()
        )
    );
}

fn empty_tools() -> Arc<RawValue> {
    Arc::from(RawValue::from_string("[]".to_string()).expect("valid tool JSON"))
}

fn prompt_with_image_outputs(detail: Option<ImageDetail>) -> Prompt {
    Prompt {
        input: vec![
            ResponseItem::Message {
                status: None,
                encrypted_content: None,
                id: None,
                role: "user".to_string(),
                content: vec![
                    ContentItem::InputText {
                        text: "Describe this image.".to_string(),
                    },
                    ContentItem::InputImage {
                        image: ImageReference::Inline {
                            image_url: "https://example.com/image.png".to_string(),
                        },
                        detail,
                    },
                ],
                phase: None,
                internal_chat_message_metadata_passthrough: None,
            },
            ResponseItem::FunctionCallOutput {
                id: None,
                call_id: Some("function-call".to_string()),
                name: None,
                namespace: None,
                output: FunctionCallOutputPayload::from_content_items(vec![
                    FunctionCallOutputContentItem::InputImage {
                        image: ImageReference::Inline {
                            image_url: "data:image/png;base64,function".to_string(),
                        },
                        detail,
                    },
                ]),
                internal_chat_message_metadata_passthrough: None,
            },
            ResponseItem::CustomToolCallOutput {
                id: None,
                call_id: "custom-call".to_string(),
                name: None,
                output: FunctionCallOutputPayload::from_content_items(vec![
                    FunctionCallOutputContentItem::InputImage {
                        image: ImageReference::Inline {
                            image_url: "data:image/png;base64,custom".to_string(),
                        },
                        detail,
                    },
                ]),
                internal_chat_message_metadata_passthrough: None,
            },
        ],
        ..Default::default()
    }
}

#[test_case::test_case(Some(ImageDetail::Original), Some(ImageDetail::High); "original")]
#[test_case::test_case(Some(ImageDetail::High), Some(ImageDetail::High); "high")]
#[test_case::test_case(Some(ImageDetail::Auto), Some(ImageDetail::Auto); "auto")]
#[test_case::test_case(Some(ImageDetail::Low), Some(ImageDetail::Low); "low")]
#[test_case::test_case(None, None; "unspecified")]
fn request_copies_project_image_details_for_receiving_model(
    detail: Option<ImageDetail>,
    unsupported_detail: Option<ImageDetail>,
) {
    let prompt = prompt_with_image_outputs(detail);
    let original = prompt.input.clone();
    let mut model_info = model_info_from_slug("gpt-5.4");
    model_info.use_responses_lite = false;
    model_info.supports_image_detail_original = true;
    assert_eq!(
        prompt.get_formatted_input_for_request(&model_info),
        original
    );

    model_info.supports_image_detail_original = false;
    assert_eq!(
        prompt.get_formatted_input_for_request(&model_info),
        prompt_with_image_outputs(unsupported_detail).input
    );

    model_info.use_responses_lite = true;
    for supports_original in [false, true] {
        model_info.supports_image_detail_original = supports_original;
        assert_eq!(
            prompt.get_formatted_input_for_request(&model_info),
            prompt_with_image_outputs(/*detail*/ None).input
        );
    }
    assert_eq!(prompt.input, original);
}

#[test]
fn serializes_text_verbosity_when_set() {
    let input: Vec<ResponseItem> = vec![];
    let req = ResponsesApiRequest {
        model: "gpt-5.4".to_string(),
        input,
        tools: Some(empty_tools().into()),
        tool_choice: "auto".to_string(),
        parallel_tool_calls: true,
        reasoning: None,
        max_output_tokens: Some(123),
        store: false,
        stream: true,
        stream_options: None,
        include: vec![],
        prompt_cache_key: None,
        service_tier: None,
        text: Some(TextControls {
            verbosity: Some(OpenAiVerbosity::Low),
            format: None,
        }),
        client_metadata: None,
        access_programs: None,
    };

    let v = serde_json::to_value(&req).expect("json");
    assert_eq!(
        v.get("text")
            .and_then(|t| t.get("verbosity"))
            .and_then(|s| s.as_str()),
        Some("low")
    );
    assert_eq!(
        v.get("max_output_tokens")
            .and_then(serde_json::Value::as_u64),
        Some(123)
    );
}

#[test]
fn serializes_text_schema_with_strict_format() {
    let input: Vec<ResponseItem> = vec![];
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "answer": {"type": "string"}
        },
        "required": ["answer"],
    });
    let text_controls = create_text_param_for_request(
        /*verbosity*/ None,
        &Some(schema.clone()),
        /*output_schema_strict*/ true,
    )
    .expect("text controls");

    let req = ResponsesApiRequest {
        model: "gpt-5.4".to_string(),
        input,
        tools: Some(empty_tools().into()),
        tool_choice: "auto".to_string(),
        parallel_tool_calls: true,
        reasoning: None,
        max_output_tokens: None,
        store: false,
        stream: true,
        stream_options: None,
        include: vec![],
        prompt_cache_key: None,
        service_tier: None,
        text: Some(text_controls),
        client_metadata: None,
        access_programs: None,
    };

    let v = serde_json::to_value(&req).expect("json");
    let text = v.get("text").expect("text field");
    assert!(text.get("verbosity").is_none());
    let format = text.get("format").expect("format field");

    assert_eq!(
        format.get("name"),
        Some(&serde_json::Value::String("codex_output_schema".into()))
    );
    assert_eq!(
        format.get("type"),
        Some(&serde_json::Value::String("json_schema".into()))
    );
    assert_eq!(format.get("strict"), Some(&serde_json::Value::Bool(true)));
    assert_eq!(format.get("schema"), Some(&schema));
}

#[test]
fn serializes_text_schema_with_non_strict_format() {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "answer": {"type": "string"},
            "rationale": {"type": "string"}
        },
        "required": ["answer"],
        "additionalProperties": false
    });
    let text_controls = create_text_param_for_request(
        /*verbosity*/ None,
        &Some(schema.clone()),
        /*output_schema_strict*/ false,
    )
    .expect("text controls");

    let format = text_controls.format.expect("format field");
    assert!(!format.strict);
    assert_eq!(format.schema, schema);
}

#[test]
fn omits_text_when_not_set() {
    let input: Vec<ResponseItem> = vec![];
    let req = ResponsesApiRequest {
        model: "gpt-5.4".to_string(),
        input,
        tools: Some(empty_tools().into()),
        tool_choice: "auto".to_string(),
        parallel_tool_calls: true,
        reasoning: None,
        max_output_tokens: None,
        store: false,
        stream: true,
        stream_options: None,
        include: vec![],
        prompt_cache_key: None,
        service_tier: None,
        text: None,
        client_metadata: None,
        access_programs: None,
    };

    let v = serde_json::to_value(&req).expect("json");
    assert!(v.get("text").is_none());
}

#[test]
fn serializes_flex_service_tier_when_set() {
    let req = ResponsesApiRequest {
        model: "gpt-5.4".to_string(),
        input: vec![],
        tools: Some(empty_tools().into()),
        tool_choice: "auto".to_string(),
        parallel_tool_calls: true,
        reasoning: None,
        max_output_tokens: None,
        store: false,
        stream: true,
        stream_options: None,
        include: vec![],
        prompt_cache_key: None,
        service_tier: Some(ServiceTier::Flex.to_string()),
        text: None,
        client_metadata: None,
        access_programs: None,
    };

    let v = serde_json::to_value(&req).expect("json");
    assert_eq!(
        v.get("service_tier").and_then(|tier| tier.as_str()),
        Some("flex")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shell_format_cache_reuses_pure_result_and_invalidates_changed_content() {
    // 完整原文作为键，晚到输出改变时不能复用旧值；共享 Arc 证明命中时没有重新格式化。
    let raw = serde_json::json!({"output":"cache-regression-first".repeat(1024),"metadata":{"exit_code":0,"duration_seconds":0.125}}).to_string();
    let raw = raw.as_str();
    let first = parse_structured_shell_output(raw).unwrap();
    let first_cached = SHELL_OUTPUT_CACHE.get(raw).unwrap().unwrap();
    assert_eq!(parse_structured_shell_output(raw).unwrap(), first);
    let second_cached = SHELL_OUTPUT_CACHE.get(raw).unwrap().unwrap();
    assert!(Arc::ptr_eq(&first_cached, &second_cached));
    let changed = raw.replace("cache-regression-first", "cache-regression-late");
    assert!(
        parse_structured_shell_output(&changed)
            .unwrap()
            .contains("cache-regression-late")
    );
    let large = serde_json::json!({"output":"x".repeat(SHELL_OUTPUT_CACHE_MAX_BYTES + 1),"metadata":{"exit_code":0,"duration_seconds":1}}).to_string();
    assert!(parse_structured_shell_output(&large).is_some());
    assert!(SHELL_OUTPUT_CACHE.get(large.as_str()).is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shell_cache_reports_repeat_work_benchmark_and_bypasses_small_outputs() {
    let small =
        r#"{"output":"small-output-bypass","metadata":{"exit_code":0,"duration_seconds":1}}"#;
    assert!(parse_structured_shell_output(small).is_some());
    assert!(SHELL_OUTPUT_CACHE.get(small).is_none());
    let raw = serde_json::json!({"output":"large shell benchmark 汉字\n".repeat(4096),"metadata":{"exit_code":0,"duration_seconds":1}}).to_string();
    let expected = build_structured_output(&serde_json::from_str::<ExecOutputJson>(&raw).unwrap());
    assert_eq!(parse_structured_shell_output(&raw).unwrap(), expected);
    let start = std::time::Instant::now();
    for _ in 0..1000 {
        std::hint::black_box(build_structured_output(
            &serde_json::from_str::<ExecOutputJson>(std::hint::black_box(&raw)).unwrap(),
        ));
    }
    let previous = start.elapsed();
    let start = std::time::Instant::now();
    for _ in 0..1000 {
        std::hint::black_box(parse_structured_shell_output(std::hint::black_box(&raw)).unwrap());
    }
    tracing::info!(
        ?previous,
        cached = ?start.elapsed(),
        "Shell repeated large output (1000 iterations)"
    );
    // 不以机器相关耗时作为通过条件；功能全等与小输入绕开缓存是确定性验收条件。
    assert_eq!(parse_structured_shell_output(&raw).unwrap(), expected);
}

#[path = "client_shell_cache_tests.rs"]
mod shell_cache_tests;
