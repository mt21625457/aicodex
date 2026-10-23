static ESTIMATES: AtomicUsize = AtomicUsize::new(0);
static TOOL_SERIALIZATIONS: AtomicUsize = AtomicUsize::new(0);
fn main() {
    let mode = std::env::args().nth(1).unwrap();
    let scenario = std::env::args().nth(2).unwrap();
    let count: usize = std::env::args().nth(3).unwrap().parse().unwrap();
    assert!(matches!(mode.as_str(), "before" | "after"));
    assert!(matches!(scenario.as_str(), "unused" | "required"));
    let catalog: codex_protocol::openai_models::ModelsResponse =
        serde_json::from_str(include_str!(__MODELS_PATH__)).unwrap();
    let mut model = catalog.models[0].clone();
    model.context_window = Some(1_000_000);
    model.max_context_window = None;
    model.max_output_tokens = if scenario == "unused" { None } else { Some(4096) };
    // 使用真正的 ResponseItem 与生产 ResponsesApiTools 序列化实现；输入构造在计时外。
    // 这是请求构建完成后的预算阶段，不包含 Prompt 克隆、工具规范化或网络。
    let input: Vec<codex_protocol::models::ResponseItem> = (0..512)
        .map(|i| serde_json::from_value(serde_json::json!({
            "type":"message", "role":if i % 2 == 0 { "user" } else { "assistant" },
            "content":[{"type":"input_text", "text":format!("message {i}: {}", "text ".repeat(400))}]
        })).unwrap())
        .collect();
    let tools_json = serde_json::json!((0..64).map(|i| serde_json::json!({
        "type":"function", "name":format!("tool_{i}"), "description":"description ".repeat(100),
        "parameters":{"type":"object", "properties":{"text":{"type":"string"}}}
    })).collect::<Vec<_>>());
    let raw: std::sync::Arc<serde_json::value::RawValue> =
        serde_json::value::to_raw_value(&tools_json).unwrap().into();
    let before_tools = Some(before::ResponsesApiTools::from(raw.clone()));
    let after_tools = Some(after::ResponsesApiTools::from(raw));
    // 同一输入逐字段相等；同时覆盖无上限/零上限/无窗口/回退窗口与饱和边界。
    for requested in [None, Some(0), Some(4096), Some(u64::MAX)] {
        for (window, fallback) in [(None,None),(None,Some(8192)),(Some(0),None),
                                  (Some(-1),None),(Some(i64::MAX),None)] {
            let mut check = model.clone();
            check.max_output_tokens = requested;
            check.context_window = window;
            check.max_context_window = fallback;
            assert_eq!(before::budget(&check, &input, &before_tools),
                       after::budget(&check, &input, &after_tools));
        }
    }
    let expected = before::budget(&model, &input, &before_tools);
    let operation = || {
        if mode == "before" { before::budget(std::hint::black_box(&model), std::hint::black_box(&input), std::hint::black_box(&before_tools)) }
        else { after::budget(std::hint::black_box(&model), std::hint::black_box(&input), std::hint::black_box(&after_tools)) }
    };
    for _ in 0..20 { assert_eq!(operation(), expected); }
    let mut times = Vec::with_capacity(count);
    ESTIMATES.store(0, Ordering::Relaxed);
    TOOL_SERIALIZATIONS.store(0, Ordering::Relaxed);
    let initial = start_allocations();
    let cpu = cpu_ns();
    let wall = std::time::Instant::now();
    for _ in 0..count {
        let started = std::time::Instant::now();
        let result = operation();
        assert_eq!(result, expected);
        std::hint::black_box(result);
        times.push(started.elapsed().as_nanos());
    }
    let wall = wall.elapsed().as_nanos();
    let cpu = cpu_ns() - cpu;
    let (calls, bytes, peak) = allocations(initial);
    let estimates = ESTIMATES.load(Ordering::Relaxed);
    let serializations = TOOL_SERIALIZATIONS.load(Ordering::Relaxed);
    let expected_count = if mode == "after" && scenario == "unused" { 0 } else { count };
    assert_eq!(estimates, expected_count);
    assert_eq!(serializations, expected_count);
    times.sort_unstable();
    println!("{{\"operations\":{count},\"estimate_calls\":{estimates},\"tool_serialization_calls\":{serializations},\"allocation_calls\":{calls},\"requested_bytes\":{bytes},\"extra_peak_live_bytes\":{peak},\"interval_cpu_ns\":{cpu},\"interval_wall_ns\":{wall},\"latency_p50_ns\":{},\"latency_p95_ns\":{}}}", times[count/2], times[count*95/100]);
}
