use codex_protocol::models::ResponseItem;

fn shell_history(count: usize, unique: usize, payload_bytes: usize, invalid: bool) -> Vec<ResponseItem> {
    (0..count).flat_map(|index| {
        let raw = if invalid {
            format!("invalid-json {} {}", index % unique, "x".repeat(payload_bytes))
        } else {
            serde_json::json!({
                "output": format!("output {}: {}", index % unique, "x".repeat(payload_bytes)),
                "metadata": {"exit_code": 0, "duration_seconds": 0.5},
            }).to_string()
        };
        [
            serde_json::from_value(serde_json::json!({
                "type": "function_call", "name": "shell", "call_id": format!("call-{index}"),
                "arguments": "{}",
            })).unwrap(),
            serde_json::from_value(serde_json::json!({
                "type": "function_call_output", "call_id": format!("call-{index}"), "output": raw,
            })).unwrap(),
        ]
    }).collect()
}

fn main() {
    let arguments: Vec<_> = std::env::args().collect();
    let scenario = &arguments[1];
    let count: usize = arguments[2].parse().unwrap();
    assert!(count > 0);
    let (outputs, unique, payload_bytes, warm, invalid) = match scenario.as_str() {
        "warm16" => (16, 16, 8192, true, false),
        "warm17" => (17, 17, 8192, true, false),
        "warm32" => (32, 32, 8192, true, false),
        "warm128" => (128, 128, 8192, true, false),
        "cold32" => (32, 32, 8192, false, false),
        "duplicate32" => (32, 1, 8192, false, false),
        "invalid32" => (32, 1, 8192, false, true),
        "small1000" => (1000, 1000, 100, false, false),
        "mixed16" => (16, 16, 8192, false, false),
        _ => panic!("unknown scenario: {scenario}"),
    };
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        let mut history = shell_history(outputs, unique, payload_bytes, invalid);
        if scenario == "mixed16" {
            // 冷缓存仅有 16 个真实 shell 候选；无关大输出不能限制其首次接纳。
            for index in 0..17 {
                history.push(serde_json::from_value(serde_json::json!({
                    "type": "function_call", "name": "unrelated",
                    "call_id": format!("other-{index}"), "arguments": "{}",
                })).unwrap());
                history.push(serde_json::from_value(serde_json::json!({
                    "type": "function_call_output", "call_id": format!("other-{index}"),
                    "output": "unrelated".repeat(1024),
                })).unwrap());
            }
        }
        let mut expected = history.clone();
        reference::convert(&mut expected);
        target::clear_cache();
        if warm {
            target::warm_cache(&history);
        }
        // 分配区间包括请求输入复制、原顺序工具配对和真实格式转换；准备 oracle 不计入。
        let mut times = Vec::with_capacity(count);
        target::reset_parses();
        let initial = start_allocations();
        let cpu = cpu_ns();
        let wall = std::time::Instant::now();
        for _ in 0..count {
            let started = std::time::Instant::now();
            let mut actual = history.clone();
            target::convert(&mut actual);
            assert_eq!(actual, expected, "history/output order must equal the baseline");
            times.push(started.elapsed().as_nanos());
        }
        let wall = wall.elapsed().as_nanos();
        let cpu = cpu_ns() - cpu;
        let (allocations, bytes, peak) = allocations(initial);
        let parses = target::parses();
        times.sort_unstable();
        // 该断言同时拒绝独立 probe 中 Tokio/cache 依赖图不匹配导致的静默禁用缓存。
        if scenario == "warm16" && VARIANT != "direct" {
            assert_eq!(parses, 0);
        }
        if scenario == "mixed16" && matches!(VARIANT, "before" | "after") {
            assert_eq!(parses, 16);
        }
        println!("{{\"operations\":{count},\"history_outputs\":{outputs},\"payload_bytes\":{payload_bytes},\"json_parses\":{parses},\"allocation_calls\":{allocations},\"requested_bytes\":{bytes},\"extra_peak_live_bytes\":{peak},\"interval_cpu_ns\":{cpu},\"interval_wall_ns\":{wall},\"latency_p50_ns\":{},\"latency_p95_ns\":{},\"latency_p99_ns\":{}}}",
            times[count / 2], times[count * 95 / 100], times[count * 99 / 100]);
    });
}
