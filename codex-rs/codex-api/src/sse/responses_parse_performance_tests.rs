use super::*;

#[test]
fn single_parse_preserves_legacy_provider_error_precedence() {
    // 用原先“兼容错误先行、类型解析随后”的算法作差分基准，涵盖错误与畸形边界。
    for data in [
        r#"{"type":"response.output_text.delta","delta":"nested error text"}"#,
        r#"{"type":"response.completed","response":{"id":"r","usage":null}}"#,
        r#"{"type":"response.failed","response":{"error":{"code":"context_length_exceeded","message":"too long"}}}"#,
        r#"{"error":{"code":"rate_limit_exceeded","message":"slow down"}}"#,
        r#"{"type":"response.error","code":"context_length_exceeded","message":"too long"}"#,
        r#"{"type":"error","error":{"code":"server_error","message":"failed"}}"#,
        r#"{"code":429,"error":"slow down"}"#,
        r#"{"type":"response.output_text.delta","delta":42,"error":{"message":"invalid"}}"#,
        r#"{"type":"error","type":"response.output_text.delta","delta":"text"}"#,
        r#"{"type":null,"error":"failed"}"#,
        r#"{"type":"unknown","error":{"message":"ignored as before"}}"#,
        r#"{"type":"response.output_text.delta","delta":null}"#,
        "null",
        "{",
        "{}",
    ] {
        let expected = if let Some(error) = provider_stream_error_from_data(data) {
            Err(error)
        } else {
            serde_json::from_str::<ResponsesStreamEvent>(data).map_err(|error| {
                provider_stream_error(
                    ProviderStreamErrorKind::ParseError,
                    format!("failed to parse Responses SSE event: {error}"),
                )
            })
        };
        assert_eq!(
            format!("{:?}", parse_responses_stream_event(data)),
            format!("{expected:?}"),
            "frame: {data}"
        );
    }
}

#[test]
fn normal_frames_require_one_text_parse_instead_of_two() {
    let data = serde_json::json!({"type":"response.output_text.delta","delta":"正文".repeat(1024)})
        .to_string();
    STREAM_FRAME_PARSE_COUNT.with(|count| count.set(0));
    for _ in 0..1000 {
        assert!(parse_responses_stream_event(&data).is_ok());
    }
    let optimized = STREAM_FRAME_PARSE_COUNT.with(std::cell::Cell::get);
    STREAM_FRAME_PARSE_COUNT.with(|count| count.set(0));
    for _ in 0..1000 {
        assert!(provider_stream_error_from_data(&data).is_none());
        assert!(deserialize_stream_frame::<ResponsesStreamEvent>(&data).is_ok());
    }
    let previous = STREAM_FRAME_PARSE_COUNT.with(std::cell::Cell::get);
    assert_eq!((optimized, previous), (1000, 2000));
    eprintln!("SSE normal-frame text parses: previous={previous}, optimized={optimized}");
}
