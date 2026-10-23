use super::*;
use pretty_assertions::assert_eq;

fn unlimited_admissions() -> usize {
    usize::MAX
}

fn shell_cache() -> ShellOutputCache {
    ShellOutputCache::new(
        std::num::NonZeroUsize::MIN.saturating_add(SHELL_OUTPUT_CACHE_CAPACITY - 1),
    )
}

fn output_json(index: usize) -> String {
    serde_json::json!({
        "output": format!("output {index}: {}", "日志🦀".repeat(1024)),
        "metadata": {"exit_code": 0, "duration_seconds": 0.5},
    })
    .to_string()
}

fn shell_history(raws: &[String]) -> Vec<ResponseItem> {
    raws.iter()
        .enumerate()
        .flat_map(|(index, raw)| {
            [
                serde_json::from_value(serde_json::json!({
                    "type": "function_call", "name": "shell",
                    "call_id": format!("call-{index}"), "arguments": "{}",
                }))
                .unwrap(),
                serde_json::from_value(serde_json::json!({
                    "type": "function_call_output", "call_id": format!("call-{index}"),
                    "output": raw,
                }))
                .unwrap(),
            ]
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unrelated_and_consumed_outputs_do_not_throttle_small_shell_working_sets() {
    for consumed in [false, true] {
        let cache = shell_cache();
        let mut history = shell_history(&(0..16).map(output_json).collect::<Vec<_>>());
        for index in 0..17 {
            history.push(
                serde_json::from_value(serde_json::json!({
                    "type": "function_call", "name": if consumed { "shell" } else { "unrelated" },
                    "call_id": format!("extra-{index}"), "arguments": "{}",
                }))
                .unwrap(),
            );
            if consumed {
                // 首个小输出仍消费配对；后续大输出不可重复计为 shell 候选。
                history.push(serde_json::from_value(serde_json::json!({
                    "type": "function_call_output", "call_id": format!("extra-{index}"), "output": "small",
                })).unwrap());
            }
            history.push(serde_json::from_value(serde_json::json!({
                "type": "function_call_output", "call_id": format!("extra-{index}"), "output": output_json(100 + index),
            })).unwrap());
        }
        let untouched_tail = history[32..].to_vec();
        for expected_parses in [
            16 + if consumed { 17 } else { 0 },
            if consumed { 17 } else { 0 },
        ] {
            let mut actual = history.clone();
            let mut budget = shell_output_cache_admission_budget(&actual);
            assert_eq!(budget, usize::MAX);
            SHELL_OUTPUT_PARSE_COUNT.with(|calls| calls.set(0));
            reserialize_shell_outputs_with_cache(&mut actual, Some(&cache), &mut budget);
            assert_eq!(
                SHELL_OUTPUT_PARSE_COUNT.with(std::cell::Cell::get),
                expected_parses
            );
            assert_eq!(&actual[32..], untouched_tail.as_slice());
            assert_ne!(&actual[..32], &history[..32]);
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn admission_pairing_supports_legacy_local_and_patch_calls() {
    for kind in [
        "shell",
        "apply_patch",
        "custom_patch",
        "local",
        "legacy_local",
    ] {
        let mut history = Vec::new();
        for index in 0..17 {
            let call_id = format!("call-{index}");
            let call = match kind {
                "custom_patch" => serde_json::json!({
                    "type": "custom_tool_call", "call_id": call_id, "name": "apply_patch", "input": "patch",
                }),
                "local" | "legacy_local" => serde_json::json!({
                    "type": "local_shell_call", "id": if kind == "legacy_local" { call_id.clone() } else { format!("ignored-{index}") },
                    "call_id": if kind == "legacy_local" { None } else { Some(&call_id) },
                    "status": "completed", "action": {"type": "exec", "command": ["true"]},
                }),
                _ => serde_json::json!({
                    "type": "function_call", "call_id": call_id, "name": kind, "arguments": "{}",
                }),
            };
            history.push(serde_json::from_value(call).unwrap());
            history.push(serde_json::from_value(serde_json::json!({
                "type": if kind == "custom_patch" { "custom_tool_call_output" } else { "function_call_output" },
                "call_id": call_id, "output": output_json(index),
            })).unwrap());
        }
        assert_eq!(shell_output_cache_admission_budget(&history), 1);
        let original = history.clone();
        SHELL_OUTPUT_PARSE_COUNT.with(|calls| calls.set(0));
        reserialize_shell_outputs_with_cache(&mut history, None, &mut unlimited_admissions());
        assert_eq!(SHELL_OUTPUT_PARSE_COUNT.with(std::cell::Cell::get), 17);
        for (index, (actual, before)) in history.iter().zip(&original).enumerate() {
            if index % 2 == 0 {
                assert_eq!(actual, before);
            } else {
                assert_ne!(actual, before);
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn admission_budget_prevents_large_history_scan_eviction() {
    // 使用真实配对/格式化路径和请求私有测试缓存，不依赖全局缓存的并发测试顺序。
    for count in [16, 17, 32, 128] {
        let cache = shell_cache();
        let raws: Vec<_> = (0..count).map(output_json).collect();
        let history = shell_history(&raws);
        let mut expected = history.clone();
        reserialize_shell_outputs_with_cache(&mut expected, None, &mut unlimited_admissions());
        // 模拟既有历史逐渐长大后，缓存已包含最后一批输出。
        reserialize_shell_outputs_with_cache(
            &mut history.clone(),
            Some(&cache),
            &mut unlimited_admissions(),
        );
        for _ in 0..20 {
            let mut actual = history.clone();
            let mut budget = shell_output_cache_admission_budget(&actual);
            SHELL_OUTPUT_PARSE_COUNT.with(|calls| calls.set(0));
            reserialize_shell_outputs_with_cache(&mut actual, Some(&cache), &mut budget);
            let parses = SHELL_OUTPUT_PARSE_COUNT.with(std::cell::Cell::get);
            assert_eq!(actual, expected);
            if count <= SHELL_OUTPUT_CACHE_CAPACITY {
                assert_eq!(parses, 0);
            } else {
                // 每次至多接纳一个 miss，至多驱逐一个将来要读取的条目。
                assert!(parses <= count - SHELL_OUTPUT_CACHE_CAPACITY + 1);
                assert!(parses < count);
                assert_eq!(budget, 0);
            }
            assert!(cache.with_mut(|entries| entries.len()) <= SHELL_OUTPUT_CACHE_CAPACITY);
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn large_repeated_outputs_keep_positive_and_negative_cache_hits() {
    for raw in [output_json(0), format!("invalid-json {}", "x".repeat(8192))] {
        let cache = shell_cache();
        let history = shell_history(&vec![raw; 32]);
        let mut expected = history.clone();
        reserialize_shell_outputs_with_cache(&mut expected, None, &mut unlimited_admissions());
        let mut actual = history.clone();
        let mut budget = shell_output_cache_admission_budget(&actual);
        assert_eq!(budget, 1);
        SHELL_OUTPUT_PARSE_COUNT.with(|calls| calls.set(0));
        reserialize_shell_outputs_with_cache(&mut actual, Some(&cache), &mut budget);
        assert_eq!(actual, expected);
        assert_eq!(SHELL_OUTPUT_PARSE_COUNT.with(std::cell::Cell::get), 1);
        assert_eq!(cache.with_mut(|entries| entries.len()), 1);
        assert_eq!(budget, 0);
    }
}

#[test]
fn admission_precheck_keeps_size_boundaries_and_disabled_runtime() {
    assert!(!shell_output_cache_size_eligible(4095));
    assert!(shell_output_cache_size_eligible(4096));
    assert!(shell_output_cache_size_eligible(1024 * 1024));
    assert!(!shell_output_cache_size_eligible(1024 * 1024 + 1));
    let history = shell_history(&[output_json(0)]);
    assert_eq!(shell_output_cache_admission_budget(&history), 0);
    let cache = shell_cache();
    let mut actual = history.clone();
    reserialize_shell_outputs_with_cache(&mut actual, Some(&cache), &mut unlimited_admissions());
    assert_ne!(actual, history);
}

#[tokio::test]
async fn current_thread_runtime_uses_uncached_formatting() {
    let history = shell_history(&[output_json(0)]);
    assert_eq!(shell_output_cache_admission_budget(&history), 0);
    let cache = shell_cache();
    let mut actual = history.clone();
    reserialize_shell_outputs_with_cache(&mut actual, Some(&cache), &mut unlimited_admissions());
    let mut expected = history;
    reserialize_shell_outputs_with_cache(&mut expected, None, &mut unlimited_admissions());
    assert_eq!(actual, expected);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn admission_policy_preserves_order_pairing_and_content_invalidation() {
    let cache = shell_cache();
    let raws: Vec<_> = (0..17).map(output_json).collect();
    let mut history = shell_history(&raws);
    // 同一个 call_id 的第二份结果仍保留原 JSON；晚到结果按原顺序消费一次配对。
    history.push(history[1].clone());
    history.push(
        serde_json::from_value(serde_json::json!({
            "type": "function_call", "name": "unrelated", "call_id": "other", "arguments": "{}",
        }))
        .unwrap(),
    );
    history.push(
        serde_json::from_value(serde_json::json!({
            "type": "function_call_output", "call_id": "other", "output": output_json(99),
        }))
        .unwrap(),
    );
    let unchanged_tail = history[history.len() - 3..].to_vec();
    for changed in [false, true] {
        let mut source = history.clone();
        if changed && let ResponseItem::FunctionCallOutput { output, .. } = &mut source[1] {
            output.body = FunctionCallOutputBody::Text(output_json(100));
        }
        let mut expected = source.clone();
        reserialize_shell_outputs_with_cache(&mut expected, None, &mut unlimited_admissions());
        let mut budget = shell_output_cache_admission_budget(&source);
        reserialize_shell_outputs_with_cache(&mut source, Some(&cache), &mut budget);
        assert_eq!(source, expected);
        assert_eq!(&source[source.len() - 3..], unchanged_tail.as_slice());
    }
}
