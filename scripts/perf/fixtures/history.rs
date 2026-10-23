use std::io::{BufRead, Seek, Write};
fn scan(path: &std::path::Path, offset: usize, expected: &str) -> usize {
    let file = std::fs::File::open(path).unwrap();
    file.lock_shared().unwrap();
    let mut cursor = history::ScanCursor::new(path, &file.metadata().unwrap(), offset, true);
    let mut reader = std::io::BufReader::new(&file);
    reader
        .seek(std::io::SeekFrom::Start(cursor.position))
        .unwrap();
    let mut line = String::new();
    let mut scanned = 0;
    loop {
        line.clear();
        let bytes = reader.read_line(&mut line).unwrap();
        assert_ne!(bytes, 0);
        scanned += 1;
        let found = cursor.row == offset;
        cursor.advance(bytes);
        if found {
            assert_eq!(line, expected);
            break;
        }
    }
    cursor.save();
    scanned
}
fn main() {
    let path = std::path::PathBuf::from(std::env::args().nth(1).unwrap());
    let rows = 2_000_000;
    {
        let mut writer = std::io::BufWriter::new(std::fs::File::create(&path).unwrap());
        for row in 0..rows {
            writeln!(writer, "消息 {row}\r").unwrap();
        }
        writer.flush().unwrap();
    }
    let expected = format!("消息 {}\r\n", rows - 1);
    assert_eq!(scan(&path, rows - 1, &expected), rows);
    let mut times = Vec::with_capacity(100);
    let mut scanned = 0;
    let initial = start_allocations();
    let cpu = cpu_ns();
    let wall = std::time::Instant::now();
    for _ in 0..100 {
        let started = std::time::Instant::now();
        scanned += scan(&path, rows - 1, &expected);
        times.push(started.elapsed().as_nanos());
    }
    let wall = wall.elapsed().as_nanos();
    let cpu = cpu_ns() - cpu;
    let (calls, bytes, peak) = allocations(initial);
    times.sort_unstable();
    println!(
        "{{\"operations\":100,\"rows\":{rows},\"scanned_lines\":{scanned},\"allocation_calls\":{calls},\"requested_bytes\":{bytes},\"extra_peak_live_bytes\":{peak},\"interval_cpu_ns\":{cpu},\"interval_wall_ns\":{wall},\"latency_p50_ns\":{},\"latency_p95_ns\":{}}}",
        times[50], times[95]
    );
}
