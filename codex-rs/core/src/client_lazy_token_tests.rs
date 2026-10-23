use super::*;
use crate::client::output_token_budget::clamp_with_lazy_input;
use pretty_assertions::assert_eq;
use std::cell::Cell;

#[test]
fn lazy_output_budget_preserves_eager_clamping_at_boundaries() {
    // 与原来的无条件估算算法作差分，覆盖缺省、零值、负窗口、最大值和饱和减法。
    // 执行次数是确定性的性能断言，不使用依赖机器速度的耗时阈值。
    for requested in [None, Some(0), Some(1), Some(4096), Some(u64::MAX)] {
        for (window, fallback, expected_calls) in [
            (None, None, 0),
            (None, Some(8192), 1),
            (Some(4096), Some(8192), 1),
            (Some(0), Some(8192), 1),
            (Some(-1), Some(8192), 1),
            (Some(i64::MIN), None, 1),
            (Some(i64::MAX), None, 1),
        ] {
            let mut model = test_model_info();
            model.max_output_tokens = requested;
            model.context_window = window;
            model.max_context_window = fallback;
            for estimate in [i64::MIN, -1, 0, 1, 4095, 4096, 8192, i64::MAX] {
                let calls = Cell::new(0);
                let actual = clamp_with_lazy_input(&model, || {
                    calls.set(calls.get() + 1);
                    estimate
                });
                assert_eq!(actual, model.clamped_max_output_tokens(Some(estimate)));
                assert_eq!(
                    calls.get(),
                    if matches!(requested, None | Some(0)) {
                        0
                    } else {
                        expected_calls
                    },
                    "requested={requested:?}, window={window:?}, fallback={fallback:?}"
                );
            }
        }
    }
}

#[test]
fn lazy_output_budget_preserves_request_payload_and_original_prompt() {
    let client = test_model_client(SessionSource::Cli);
    let prompt = Prompt {
        tools: vec![ToolSpec::Freeform(codex_tools::FreeformTool {
            name: "exec".to_string(),
            description: "Execute JavaScript.".to_string(),
            defer_loading: None,
            format: codex_tools::FreeformToolFormat {
                r#type: "grammar".to_string(),
                syntax: "lark".to_string(),
                definition: "start: /.+/".to_string(),
            },
        })]
        .into(),
        input: vec![
            serde_json::from_value(json!({
                "type": "message", "role": "user",
                "content": [{"type": "input_text", "text": "large history\n".repeat(1024)}]
            }))
            .unwrap(),
        ],
        ..Default::default()
    };
    let original_input = prompt.input.clone();
    let metadata = test_responses_metadata_for_client(
        &client,
        None,
        format!("{}:0", client.state.thread_id),
        None,
        TestCodexResponsesRequestKind::Turn,
    );
    for responses_lite in [false, true] {
        let mut payload_without_budget = None;
        for (requested, window, fallback) in [
            (None, Some(8192), None),
            (Some(0), Some(8192), None),
            (Some(4096), None, None),
            (Some(4096), None, Some(8192)),
            (Some(4096), Some(0), Some(8192)),
            (Some(u64::MAX), Some(i64::MAX), None),
        ] {
            let mut model = test_model_info();
            model.use_responses_lite = responses_lite;
            model.max_output_tokens = requested;
            model.context_window = window;
            model.max_context_window = fallback;
            let request = client
                .build_responses_request(
                    &prompt,
                    &model,
                    None,
                    codex_protocol::config_types::ReasoningSummary::None,
                    None,
                    &metadata,
                    true,
                )
                .unwrap();
            // 从真实构建后的协议输入计算旧算法结果，同时覆盖 Responses Lite 把工具
            // 放入 AdditionalTools 的路径，避免只验证普通 tools 字段的估算。
            let eager_estimate =
                crate::context_manager::estimate_request_input_tokens("", &request.input)
                    .saturating_add(
                        request
                            .tools
                            .as_ref()
                            .map(crate::context_manager::estimate_serialized_tokens)
                            .unwrap_or(0),
                    );
            assert_eq!(
                request.max_output_tokens,
                model.clamped_max_output_tokens(Some(eager_estimate))
            );
            let mut payload = serde_json::to_value(&request).unwrap();
            payload.as_object_mut().unwrap().remove("max_output_tokens");
            if let Some(expected) = &payload_without_budget {
                assert_eq!(&payload, expected);
            } else {
                payload_without_budget = Some(payload);
            }
            assert_eq!(prompt.input, original_input);
        }
    }
}
