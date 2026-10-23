use super::*;
use serde_json::json;

#[test]
fn counting_writer_matches_materialized_json_byte_metrics() {
    // 逐字节对照旧算法：UTF-8不是字符数，控制字符/引号也不能直接按源文本长度估算。
    for results in [
        vec![],
        vec![json!(null), json!(true), json!(false)],
        vec![
            json!({"title":"中文🦀", "snippet":"line\n\t\r\u{0000}\u{001f}\"\\", "url":"https://例子.example/路径"}),
        ],
        vec![
            json!([0, -1, i64::MIN, i64::MAX]),
            json!(u64::MAX),
            json!(1.25e100),
        ],
        vec![json!({"nested":[{"a":[1,2,3]}, {"b":{}}]})],
        vec![json!({"snippet":"大结果\n\"🦀".repeat(100_000)})],
    ] {
        let previous = serde_json::to_vec(&results).unwrap();
        assert_eq!(
            results_payload_bytes(&results).unwrap(),
            i64::try_from(previous.len()).unwrap()
        );
    }
}

#[test]
fn counting_writer_keeps_constant_storage_for_fragmented_output() {
    use std::io::Write;
    let mut counter = JsonByteCounter::default();
    for _ in 0..100_000 {
        counter.write_all(b"fragment").unwrap();
    }
    counter.flush().unwrap();
    assert_eq!(counter.bytes, 800_000);
    assert_eq!(
        std::mem::size_of_val(&counter),
        std::mem::size_of::<usize>()
    );
    // 计数器不保留任何输出缓冲；测试同时验证 writer 完整消费每个分块。
}
