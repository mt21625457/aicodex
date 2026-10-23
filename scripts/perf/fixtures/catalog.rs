fn lookup(mode: &str) -> codex_protocol::openai_models::ModelInfo {
    if mode == "before" {
        before::bundled_models_response()
            .unwrap()
            .models
            .into_iter()
            .find(|model| model.slug == "gpt-6.1-sol")
            .unwrap()
    } else {
        after::bundled_models_response_ref()
            .unwrap()
            .models
            .iter()
            .find(|model| model.slug == "gpt-6.1-sol")
            .cloned()
            .unwrap()
    }
}
fn main() {
    let mode = std::env::args().nth(1).unwrap();
    assert!(matches!(mode.as_str(), "before" | "after"));
    assert_eq!(lookup("before"), lookup("after"));
    let count = 20_000;
    let mut times = Vec::with_capacity(count);
    for _ in 0..100 {
        std::hint::black_box(lookup(&mode));
    }
    let initial = start_allocations();
    let cpu = cpu_ns();
    let wall = std::time::Instant::now();
    for _ in 0..count {
        let started = std::time::Instant::now();
        std::hint::black_box(lookup(&mode));
        times.push(started.elapsed().as_nanos());
    }
    let wall = wall.elapsed().as_nanos();
    let cpu = cpu_ns() - cpu;
    let (calls, bytes, peak) = allocations(initial);
    times.sort_unstable();
    println!(
        "{{\"operations\":{count},\"allocation_calls\":{calls},\"requested_bytes\":{bytes},\"extra_peak_live_bytes\":{peak},\"interval_cpu_ns\":{cpu},\"interval_wall_ns\":{wall},\"latency_p50_ns\":{},\"latency_p95_ns\":{}}}",
        times[count / 2],
        times[count * 95 / 100]
    );
}
