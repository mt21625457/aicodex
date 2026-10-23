const MAX_FRAGMENT_BYTES: u64 = 2 * 1024 * 1024;
fn main() {
    let evidence = std::path::PathBuf::from(std::env::args().nth(1).unwrap());
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    let path = root.join("large.html");
    let fragment = "x".repeat(1024 * 1024);
    std::fs::write(&path, &fragment).unwrap();
    let viewer_dir = root.join("viewers");
    let cache = std::sync::Mutex::new(std::collections::HashMap::new());
    let output = viewer::materialize_document(&path, &viewer_dir, &cache).unwrap();
    let document = std::fs::read(&output).unwrap();
    let modified = std::fs::metadata(&output).unwrap().modified().unwrap();
    let mut times = Vec::with_capacity(200);
    let initial = start_allocations();
    let cpu = cpu_ns();
    let wall = std::time::Instant::now();
    for _ in 0..200 {
        let started = std::time::Instant::now();
        let hit = viewer::materialize_document(&path, &viewer_dir, &cache).unwrap();
        assert_eq!(hit, output);
        times.push(started.elapsed().as_nanos());
    }
    let wall = wall.elapsed().as_nanos();
    let cpu = cpu_ns() - cpu;
    let (calls, bytes, peak) = allocations(initial);
    times.sort_unstable();
    assert_eq!(document, std::fs::read(&output).unwrap());
    assert_eq!(
        modified,
        std::fs::metadata(&output).unwrap().modified().unwrap()
    );
    // 父进程逐字比较两版生成文件，不能仅用相同路径或长度作为等价证明。
    std::fs::write(evidence, &document).unwrap();
    println!(
        "{{\"operations\":200,\"fragment_bytes\":{},\"allocation_calls\":{calls},\"requested_bytes\":{bytes},\"extra_peak_live_bytes\":{peak},\"interval_cpu_ns\":{cpu},\"interval_wall_ns\":{wall},\"latency_p50_ns\":{},\"latency_p95_ns\":{}}}",
        fragment.len(),
        times[100],
        times[190]
    );
}
